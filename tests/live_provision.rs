use std::io::{self, Read};

#[test]
#[ignore = "modifies the webos-mcp credential store; requires privately verified input"]
fn provision_existing_authorization_for_live_test() {
    let mut record = String::new();
    io::stdin()
        .read_to_string(&mut record)
        .expect("private input");
    let value: serde_json::Value = serde_json::from_str(&record).expect("credential record");
    assert_eq!(value["fingerprint"].as_array().map(Vec::len), Some(32));
    assert!(
        value["client_key"]
            .as_str()
            .is_some_and(|key| !key.is_empty())
    );
    assert!(value["ssh"].is_object());
    let entry = keyring::Entry::new("webos-mcp", "tv-credentials").unwrap();
    assert!(
        matches!(entry.get_password(), Err(keyring::Error::NoEntry)),
        "existing credentials must not be overwritten"
    );
    entry
        .set_password(&record)
        .expect("save to credential store");
    assert!(entry.get_password().expect("read credential store") == record);
}
