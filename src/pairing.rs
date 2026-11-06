//! Registration messages; transport authentication is the caller's responsibility.

use serde_json::{Value, json};

const REGISTER_ID: &str = "register_0";
const MAX_RESPONSE_BYTES: usize = 32 * 1024;

/// A pairing response without exposing its raw contents through diagnostics.
pub enum Registration {
    Prompt,
    Registered(String),
    Rejected,
}

/// Encode a request only after the selected TV's identity has been verified.
/// The returned bytes may contain a credential and must never be logged.
pub fn registration_request(client_key: Option<&str>) -> String {
    let mut payload = json!({
        "forcePairing": false,
        "pairingType": "PROMPT",
        "manifest": {
            "manifestVersion": 1,
            "appVersion": "1.0",
            "permissions": ["CONTROL_MOUSE_AND_KEYBOARD", "LAUNCH", "READ_APP_STATUS"]
        }
    });
    if let Some(key) = client_key {
        payload["client-key"] = Value::String(key.to_owned());
    }
    json!({"id": REGISTER_ID, "type": "register", "payload": payload}).to_string()
}

/// Parse only a matching registration response; return no raw remote error details.
pub fn parse_registration(bytes: &[u8]) -> Result<Registration, &'static str> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("TV registration response exceeded the size limit");
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| "Invalid TV registration response")?;
    if value.get("id").and_then(Value::as_str) != Some(REGISTER_ID) {
        return Err("Unexpected TV registration response");
    }
    match value.get("type").and_then(Value::as_str) {
        Some("response")
            if value
                .pointer("/payload/pairingType")
                .and_then(Value::as_str)
                == Some("PROMPT") =>
        {
            Ok(Registration::Prompt)
        }
        Some("registered") => {
            let key = value
                .pointer("/payload/client-key")
                .and_then(Value::as_str)
                .filter(|key| !key.is_empty() && key.len() <= 4096)
                .ok_or("Invalid TV registration response")?;
            Ok(Registration::Registered(key.to_owned()))
        }
        Some("error") => Ok(Registration::Rejected),
        _ => Err("Unexpected TV registration response"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_excludes_key_on_initial_pairing_and_includes_on_reconnect() {
        let initial: Value = serde_json::from_str(&registration_request(None)).unwrap();
        assert_eq!(initial["type"], "register");
        assert_eq!(initial["payload"]["pairingType"], "PROMPT");
        assert!(initial["payload"].get("client-key").is_none());
        let resumed: Value =
            serde_json::from_str(&registration_request(Some("synthetic-key"))).unwrap();
        assert_eq!(resumed["payload"]["client-key"], "synthetic-key");
    }

    #[test]
    fn parse_prompt_success_and_rejection_without_exposing_errors() {
        assert!(matches!(
            parse_registration(
                br#"{"id":"register_0","type":"response","payload":{"pairingType":"PROMPT"}}"#
            ),
            Ok(Registration::Prompt)
        ));
        match parse_registration(
            br#"{"id":"register_0","type":"registered","payload":{"client-key":"synthetic-key"}}"#,
        ) {
            Ok(Registration::Registered(key)) => assert_eq!(key, "synthetic-key"),
            _ => panic!("expected pairing success"),
        }
        assert!(matches!(
            parse_registration(
                br#"{"id":"register_0","type":"error","error":"private remote payload"}"#
            ),
            Ok(Registration::Rejected)
        ));
        assert!(
            parse_registration(
                br#"{"id":"other","type":"registered","payload":{"client-key":"synthetic-key"}}"#
            )
            .is_err()
        );
        assert!(
            parse_registration(
                br#"{"id":"register_0","type":"registered","payload":{"client-key":""}}"#
            )
            .is_err()
        );
        assert!(parse_registration(&vec![b' '; MAX_RESPONSE_BYTES + 1]).is_err());
    }
}
