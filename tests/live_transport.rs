use std::io::{self, Read};
use webos_mcp::{metrics::Metrics, transport};

#[tokio::test]
#[ignore = "requires an explicitly configured TV and verified certificate via private stdin"]
async fn connect_with_verified_certificate_and_existing_key() {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .expect("private input");
    let mut lines = input.lines();
    let endpoint = lines
        .next()
        .expect("endpoint")
        .parse()
        .expect("valid endpoint");
    let digest = lines.next().expect("verified fingerprint");
    assert_eq!(digest.len(), 64, "invalid fingerprint length");
    let mut pin = [0_u8; 32];
    for (i, byte) in pin.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digest[i * 2..i * 2 + 2], 16).expect("valid fingerprint");
    }
    let key = lines.next().expect("pairing key");
    let mut socket = transport::connect(endpoint, pin, &Metrics::new(false))
        .await
        .expect("authenticated connection");
    match transport::register(&mut socket, Some(key)).await {
        Ok(returned_key) => assert!(returned_key == key, "TV returned a different key"),
        Err(error) => panic!("registration failed: {error}"),
    }
    let mut input = match socket.input_socket(endpoint, &Metrics::new(false)).await {
        Ok(value) => value,
        Err(error) => panic!("input socket failed: {error}"),
    };
    if std::env::var_os("WEBOS_MCP_TEST_IDLE").is_some() {
        tokio::time::sleep(std::time::Duration::from_secs(50)).await;
    }
    assert!(
        input.probe().await.is_ok(),
        "input connection did not answer a ping"
    );
}
