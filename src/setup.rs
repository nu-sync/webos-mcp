use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use webos_mcp::{
    capture::{CaptureConfig, CaptureSession},
    metrics::Metrics,
    transport,
};

const SERVICE: &str = "webos-mcp";
const ACCOUNT: &str = "tv-address";

fn read_legacy_address() -> Result<Option<IpAddr>, &'static str> {
    let entry = keyring::Entry::new(SERVICE, ACCOUNT)
        .map_err(|_| "Secure credential store is unavailable.")?;
    match entry.get_password() {
        Ok(value) => value
            .parse()
            .map(Some)
            .map_err(|_| "Saved TV address is invalid."),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err("Secure credential store is unavailable."),
    }
}

fn parse_address(input: &str) -> Result<Option<IpAddr>, &'static str> {
    let address = input.trim();
    if address.is_empty() {
        return Ok(None);
    }
    IpAddr::from_str(address)
        .map(Some)
        .map_err(|_| "Enter a valid IPv4 or IPv6 address.")
}

#[derive(Serialize, Deserialize)]
pub struct Credentials {
    #[serde(default)]
    pub address: Option<IpAddr>,
    pub fingerprint: [u8; 32],
    pub client_key: String,
    #[serde(default)]
    pub ssh: Option<SshConfig>,
}

#[derive(Serialize, Deserialize)]
pub struct SshConfig {
    pub port: u16,
    pub username: String,
    pub agent_socket: String,
    pub public_key: String,
}

pub fn load_address() -> Result<IpAddr, &'static str> {
    read_legacy_address()?.ok_or("Run setup to configure the TV address.")
}

fn credentials_entry() -> Result<keyring::Entry, &'static str> {
    keyring::Entry::new(SERVICE, "tv-credentials")
        .map_err(|_| "Secure credential store is unavailable.")
}

pub fn load_credentials() -> Result<Option<Credentials>, &'static str> {
    let value = match crate::local_store::read()? {
        Some(value) => {
            crate::local_store::remember_choice()?;
            Some(value)
        }
        None => {
            if crate::local_store::chosen()? {
                return Err(
                    "Local credential file is missing; restore it or remove and reconfigure the device.",
                );
            }
            match credentials_entry()?.get_password() {
                Ok(value) => Some(value),
                Err(keyring::Error::NoEntry) => None,
                Err(_) => return Err("Secure credential store is unavailable."),
            }
        }
    };
    match value {
        Some(value) => {
            let parsed: Credentials = serde_json::from_str(&value)
                .map_err(|_| "Saved TV credentials are invalid; re-pair the TV.")?;
            if parsed.client_key.is_empty() || parsed.client_key.len() > 4096 {
                return Err("Saved TV credentials are invalid; re-pair the TV.");
            }
            Ok(Some(parsed))
        }
        None => Ok(None),
    }
}

fn save_credentials(record: &Credentials, verify: bool) -> Result<(), &'static str> {
    let encoded = serde_json::to_string(record).map_err(|_| "Could not encode credentials.")?;
    if crate::local_store::read()?.is_some() {
        crate::local_store::save(&encoded, true)?;
        if verify && crate::local_store::read()?.as_deref() != Some(encoded.as_str()) {
            return Err("Could not verify saved credentials.");
        }
    } else {
        if crate::local_store::chosen()? {
            return Err(
                "Local credential file is missing; restore it or remove and reconfigure the device.",
            );
        }
        let entry = credentials_entry()?;
        entry
            .set_password(&encoded)
            .map_err(|_| "Could not save credentials.")?;
        if verify
            && entry
                .get_password()
                .map_err(|_| "Could not verify saved credentials.")?
                != encoded
        {
            return Err("Could not verify saved credentials.");
        }
    }
    Ok(())
}

pub fn load_device_credentials() -> Result<(IpAddr, Credentials), &'static str> {
    let credentials = load_credentials()?.ok_or("Run setup to pair the TV")?;
    crate::session::diagnostic("TV credential entry loaded");
    let address = match credentials.address {
        Some(address) => address,
        None => {
            if crate::local_store::read()?.is_some() {
                return Err("Local credentials have no TV address.");
            }
            crate::session::diagnostic("loading legacy address");
            load_address()?
        }
    };
    crate::session::diagnostic("TV address loaded");
    Ok((address, credentials))
}

pub fn persist_verified_credentials(
    credentials: &mut Credentials,
    address: IpAddr,
    key: String,
) -> Result<(), &'static str> {
    update_verified_credentials(credentials, address, key, |record| {
        save_credentials(record, false)
    })
}

fn update_verified_credentials(
    credentials: &mut Credentials,
    address: IpAddr,
    key: String,
    save: impl FnOnce(&Credentials) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    if credentials.address == Some(address) && credentials.client_key == key {
        return Ok(());
    }
    let previous_address = credentials.address;
    let previous_key = std::mem::replace(&mut credentials.client_key, key);
    credentials.address = Some(address);
    if let Err(error) = save(credentials) {
        credentials.address = previous_address;
        credentials.client_key = previous_key;
        return Err(error);
    }
    Ok(())
}

fn fingerprint(input: &str) -> Result<[u8; 32], &'static str> {
    let text: String = input.trim().chars().filter(|c| *c != ':').collect();
    if text.len() != 64 || !text.is_ascii() {
        return Err("Enter a SHA-256 fingerprint as 64 hexadecimal digits.");
    }
    let mut bytes = [0; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16)
            .map_err(|_| "Enter a SHA-256 fingerprint as 64 hexadecimal digits.")?;
    }
    Ok(bytes)
}

async fn authorize_and_save<F, Fut, S>(
    address: IpAddr,
    pin: [u8; 32],
    old: Option<Credentials>,
    re_pair: bool,
    authorize: F,
    save: S,
) -> Result<Credentials, &'static str>
where
    F: FnOnce(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<String, &'static str>>,
    S: FnOnce(&Credentials) -> Result<(), &'static str>,
{
    if old.as_ref().is_some_and(|value| value.fingerprint != pin) {
        return Err("TV fingerprint changed; existing credentials were not sent or changed.");
    }
    let existing_key = if re_pair {
        None
    } else {
        old.as_ref().map(|value| value.client_key.clone())
    };
    let key = authorize(existing_key).await?;
    let unchanged = old.as_ref().is_some_and(|value| {
        value.client_key == key && value.fingerprint == pin && value.address == Some(address)
    });
    let record = Credentials {
        address: Some(address),
        fingerprint: pin,
        client_key: key,
        ssh: old.and_then(|value| value.ssh),
    };
    if !unchanged {
        save(&record)?;
    }
    Ok(record)
}

pub async fn setup(re_pair: bool) -> Result<(), &'static str> {
    let _lock = crate::session::acquire_lock()?;
    eprintln!("Enter the TV IP address from its network settings. Input is hidden.");
    eprintln!("If an address is already saved, press Enter to keep it.");
    let address = rpassword::prompt_password("TV IP address: ")
        .map_err(|_| "Could not read input from a terminal; no address was changed.")?;
    let candidate = parse_address(&address)?;
    eprintln!("Checking saved credentials; a system access prompt may appear.");
    let old = load_credentials()?;
    let address = match candidate.or_else(|| old.as_ref().and_then(|value| value.address)) {
        Some(value) => value,
        None => load_address()?,
    };
    eprintln!(
        "Enter the TV certificate SHA-256 fingerprint verified through a trusted independent channel. Input is hidden."
    );
    if old.is_some() {
        eprintln!("Press Enter to retain the previously verified fingerprint.");
    }
    let input = rpassword::prompt_password("Verified fingerprint: ")
        .map_err(|_| "Could not read input from a terminal.")?;
    let pin = if input.trim().is_empty() {
        old.as_ref()
            .map(|value| value.fingerprint)
            .ok_or("A verified fingerprint is required.")?
    } else {
        fingerprint(&input)?
    };
    if re_pair || old.is_none() {
        eprintln!("Approve the pairing request on the TV if prompted.");
    }
    let mut record = authorize_and_save(
        address,
        pin,
        old,
        re_pair,
        |key| async move {
            let mut socket =
                transport::connect(SocketAddr::new(address, 3001), pin, &Metrics::new(false))
                    .await?;
            transport::register(&mut socket, key.as_deref()).await
        },
        |record| save_credentials(record, true),
    )
    .await?;
    println!("TV address and pairing saved.");
    if let Some(ssh) = &record.ssh {
        verify_capture(address, ssh).await?;
        println!("Existing SSH capture configuration verified.");
        return Ok(());
    }
    eprintln!(
        "For a rooted TV with existing SSH access, enter its SSH user. Press Enter to skip capture setup."
    );
    let username = rpassword::prompt_password("SSH user: ")
        .map_err(|_| "Could not read input from a terminal.")?;
    if username.trim().is_empty() {
        return Err("Visual capture is not configured; setup is incomplete.");
    }
    let port_input = rpassword::prompt_password("SSH port (Enter for 22): ")
        .map_err(|_| "Could not read input from a terminal.")?;
    let port = if port_input.trim().is_empty() {
        22
    } else {
        port_input
            .trim()
            .parse::<u16>()
            .map_err(|_| "Invalid SSH port.")?
    };
    if port == 0 {
        return Err("Invalid SSH port.");
    }
    let agent_socket = rpassword::prompt_password("SSH agent socket or pipe: ")
        .map_err(|_| "Could not read input from a terminal.")?;
    let public_key_path = rpassword::prompt_password("SSH public key file: ")
        .map_err(|_| "Could not read input from a terminal.")?;
    let public_key = std::fs::read_to_string(public_key_path.trim())
        .map_err(|_| "Could not read SSH public key file.")?;
    let ssh = SshConfig {
        port,
        username: username.trim().to_owned(),
        agent_socket: agent_socket.trim().to_owned(),
        public_key: public_key.trim().to_owned(),
    };
    verify_capture(address, &ssh).await?;
    record.ssh = Some(ssh);
    save_credentials(&record, true)?;
    println!(
        "SSH capture is configured. Video and external inputs may not appear in compositor screenshots."
    );
    Ok(())
}

fn remove() -> Result<(), &'static str> {
    let _lock = crate::session::acquire_lock()?;
    for account in ["tv-credentials", ACCOUNT] {
        let entry = keyring::Entry::new(SERVICE, account)
            .map_err(|_| "Secure credential store is unavailable.")?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(_) => return Err("Could not remove all local TV credentials; retry removal."),
        }
    }
    crate::local_store::remove()?;
    println!(
        "Local TV address and credentials removed. TV-side authorization must be revoked separately. Any unresolved action remains blocked."
    );
    Ok(())
}

async fn migrate_local() -> Result<(), &'static str> {
    let _lock = crate::session::acquire_lock()?;
    if crate::local_store::read()?.is_some() {
        load_credentials()?.ok_or("Local credentials are invalid.")?;
        println!("Local credential file is already configured.");
        return Ok(());
    }
    eprintln!(
        "Reading existing pairing and address; the system credential store may request access."
    );
    let (address, mut record) = load_device_credentials()?;
    eprintln!("Verifying the TV before saving local credentials.");
    let mut remote = transport::connect(
        SocketAddr::new(address, 3001),
        record.fingerprint,
        &Metrics::new(false),
    )
    .await?;
    record.client_key = transport::register(&mut remote, Some(&record.client_key)).await?;
    record.address = Some(address);
    let encoded = serde_json::to_string(&record).map_err(|_| "Could not encode credentials.")?;
    crate::local_store::save(&encoded, false)?;
    crate::local_store::remember_choice()?;
    if crate::local_store::read()?.as_deref() != Some(encoded.as_str()) {
        return Err("Could not verify local credentials.");
    }
    println!(
        "Private local credential file configured. Existing system credentials were retained."
    );
    Ok(())
}

async fn import_local() -> Result<(), &'static str> {
    use std::io::Read;
    let _lock = crate::session::acquire_lock()?;
    if crate::local_store::read()?.is_some() {
        return Err("Local credentials already exist; they were not changed.");
    }
    let mut input = String::new();
    std::io::stdin()
        .take(64 * 1024 + 1)
        .read_to_string(&mut input)
        .map_err(|_| "Could not read credential input.")?;
    if input.len() > 64 * 1024 {
        return Err("Credential input exceeds the size limit.");
    }
    let mut record: Credentials =
        serde_json::from_str(&input).map_err(|_| "Invalid credential input.")?;
    let address = record.address.ok_or("A TV address is required.")?;
    if record.client_key.is_empty() || record.client_key.len() > 4096 {
        return Err("Invalid credential input.");
    }
    eprintln!("Verifying the TV before saving local credentials.");
    let mut remote = transport::connect(
        SocketAddr::new(address, 3001),
        record.fingerprint,
        &Metrics::new(false),
    )
    .await?;
    record.client_key = transport::register(&mut remote, Some(&record.client_key)).await?;
    let encoded = serde_json::to_string(&record).map_err(|_| "Could not encode credentials.")?;
    crate::local_store::save(&encoded, false)?;
    crate::local_store::remember_choice()?;
    if crate::local_store::read()?.as_deref() != Some(encoded.as_str()) {
        return Err("Could not verify local credentials.");
    }
    println!("Private local credential file configured.");
    Ok(())
}

async fn verify_capture(address: IpAddr, ssh: &SshConfig) -> Result<(), &'static str> {
    let config = CaptureConfig {
        endpoint: SocketAddr::new(address, ssh.port),
        username: ssh.username.clone(),
        agent_socket: ssh.agent_socket.clone(),
        public_key: ssh.public_key.clone(),
    };
    eprintln!(
        "Checking trusted SSH access and visual capture; approve SSH agent access if prompted."
    );
    let mut capture = CaptureSession::connect(&config).await?;
    capture.capture().await.map_err(|error| error.message())?;
    capture.close().await
}

pub async fn run_cli() -> Result<bool, &'static str> {
    let mut args = std::env::args_os().skip(1);
    match (args.next(), args.next()) {
        (None, None) => Ok(false),
        (Some(command), None) if command == "setup" => {
            setup(false).await?;
            Ok(true)
        }
        (Some(command), Some(option)) if command == "setup" && option == "--re-pair" => {
            setup(true).await?;
            Ok(true)
        }
        (Some(command), None) if command == "remove" => {
            remove()?;
            Ok(true)
        }
        (Some(command), None) if command == "use-local-file" => {
            migrate_local().await?;
            Ok(true)
        }
        (Some(command), Some(option))
            if command == "use-local-file" && option == "--import-stdin" =>
        {
            import_local().await?;
            Ok(true)
        }
        (Some(command), None) if command == "--help" || command == "-h" => {
            println!(
                "Usage: webos-mcp [setup [--re-pair] | use-local-file | remove]\n\nRun without arguments to start the MCP stdio server.\nRun use-local-file to copy existing paired credentials into an unencrypted local file."
            );
            Ok(true)
        }
        _ => Err("Usage: webos-mcp [setup [--re-pair] | use-local-file | remove]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_credentials() -> Credentials {
        Credentials {
            address: Some("192.0.2.1".parse().unwrap()),
            fingerprint: [1; 32],
            client_key: "synthetic".into(),
            ssh: None,
        }
    }

    #[tokio::test]
    async fn failed_verification_preserves_the_previous_address_and_credentials() {
        let stored =
            std::cell::RefCell::new(serde_json::to_string(&synthetic_credentials()).unwrap());
        let old: Credentials = serde_json::from_str(&stored.borrow()).unwrap();
        let result = authorize_and_save(
            "192.0.2.2".parse().unwrap(),
            [1; 32],
            Some(old),
            false,
            |_| async { Err("TV TLS authentication failed") },
            |record| {
                *stored.borrow_mut() = serde_json::to_string(record).unwrap();
                Ok(())
            },
        )
        .await;
        assert!(matches!(result, Err("TV TLS authentication failed")));
        let saved: Credentials = serde_json::from_str(&stored.borrow()).unwrap();
        assert_eq!(saved.address, Some("192.0.2.1".parse().unwrap()));
        assert_eq!(saved.client_key, "synthetic");
    }

    #[tokio::test]
    async fn changed_fingerprint_rejects_before_contact_or_store() {
        let result = authorize_and_save(
            "192.0.2.2".parse().unwrap(),
            [2; 32],
            Some(synthetic_credentials()),
            false,
            |_| async { panic!("untrusted TV must not be contacted") },
            |_| panic!("credentials must not be overwritten"),
        )
        .await;
        assert!(matches!(
            result,
            Err("TV fingerprint changed; existing credentials were not sent or changed.")
        ));
    }

    #[tokio::test]
    async fn verified_address_and_pairing_commit_together() {
        let stored = std::cell::RefCell::new(String::new());
        let result = authorize_and_save(
            "192.0.2.2".parse().unwrap(),
            [1; 32],
            Some(synthetic_credentials()),
            false,
            |key| async move {
                assert_eq!(key.as_deref(), Some("synthetic"));
                Ok("synthetic".into())
            },
            |record| {
                *stored.borrow_mut() = serde_json::to_string(record).unwrap();
                Ok(())
            },
        )
        .await;
        assert!(result.is_ok());
        let saved: Credentials = serde_json::from_str(&stored.borrow()).unwrap();
        assert_eq!(saved.address, Some("192.0.2.2".parse().unwrap()));
        assert_eq!(saved.client_key, "synthetic");
    }
    #[test]
    fn address_input_is_validated_without_store_access() {
        assert_eq!(parse_address(" "), Ok(None));
        assert_eq!(
            parse_address("192.0.2.1"),
            Ok(Some("192.0.2.1".parse().unwrap()))
        );
        let invalid = "not-an-ip";
        let error = parse_address(invalid).unwrap_err();
        assert_eq!(error, "Enter a valid IPv4 or IPv6 address.");
        assert!(!error.contains(invalid));
    }

    #[test]
    fn credentials_accept_legacy_and_combined_records() {
        let legacy = serde_json::json!({"fingerprint": vec![0u8; 32], "client_key": "synthetic"});
        let parsed: Credentials = serde_json::from_value(legacy).unwrap();
        assert_eq!(parsed.address, None);
        let combined = Credentials {
            address: Some("192.0.2.1".parse().unwrap()),
            fingerprint: [0; 32],
            client_key: "synthetic".into(),
            ssh: None,
        };
        let parsed: Credentials =
            serde_json::from_str(&serde_json::to_string(&combined).unwrap()).unwrap();
        assert_eq!(parsed.address, Some("192.0.2.1".parse().unwrap()));
        assert_eq!(parsed.client_key, "synthetic");
    }

    #[test]
    fn verified_legacy_credentials_migrate_only_once() {
        let mut credentials = Credentials {
            address: None,
            fingerprint: [0; 32],
            client_key: "synthetic".into(),
            ssh: None,
        };
        let mut saved = String::new();
        let address = "192.0.2.1".parse().unwrap();
        update_verified_credentials(&mut credentials, address, "new-key".into(), |value| {
            saved = serde_json::to_string(value).unwrap();
            Ok(())
        })
        .unwrap();
        let mut restarted: Credentials = serde_json::from_str(&saved).unwrap();
        assert_eq!(restarted.address, Some(address));
        assert_eq!(restarted.client_key, "new-key");
        assert_eq!(
            update_verified_credentials(&mut restarted, address, "new-key".into(), |_| Err(
                "unexpected store write"
            )),
            Ok(())
        );
    }

    #[test]
    fn failed_credential_consolidation_preserves_in_memory_values() {
        let mut credentials = Credentials {
            address: None,
            fingerprint: [0; 32],
            client_key: "synthetic".into(),
            ssh: None,
        };
        assert_eq!(
            update_verified_credentials(
                &mut credentials,
                "192.0.2.1".parse().unwrap(),
                "new-key".into(),
                |_| Err("store unavailable")
            ),
            Err("store unavailable")
        );
        assert_eq!(credentials.address, None);
        assert_eq!(credentials.client_key, "synthetic");
    }
}
