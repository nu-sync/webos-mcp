//! Opt-in OS credential-store test. It writes only a synthetic address and removes it.
use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

const SERVICE: &str = "webos-mcp-test";
const SAMPLE: &str = "192.0.2.42";

struct Cleanup(keyring::Entry);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.0.delete_credential();
    }
}

#[test]
#[ignore = "requires an available unlocked OS credential store"]
fn synthetic_address_survives_process_restart() {
    let id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let entry = Cleanup(keyring::Entry::new(SERVICE, &id).unwrap());
    entry
        .0
        .set_password(SAMPLE)
        .expect("secure test store unavailable");
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "read_in_child"])
        .env("WEBOS_TEST_ACCOUNT", &id)
        .status()
        .expect("test child failed to start");
    assert!(
        status.success(),
        "test child could not read the stored synthetic value"
    );
    entry
        .0
        .delete_credential()
        .expect("test credential cleanup failed");
    assert!(matches!(
        entry.0.get_password(),
        Err(keyring::Error::NoEntry)
    ));
}

#[test]
#[ignore = "helper launched by synthetic_address_survives_process_restart"]
fn read_in_child() {
    let id = std::env::var("WEBOS_TEST_ACCOUNT").expect("test account required");
    let value = keyring::Entry::new(SERVICE, &id)
        .unwrap()
        .get_password()
        .expect("test credential unavailable");
    assert_eq!(value, SAMPLE);
}
