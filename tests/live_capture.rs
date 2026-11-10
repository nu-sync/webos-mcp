use std::io::{self, Read};
use webos_mcp::capture::{CaptureConfig, CaptureSession};

#[tokio::test]
#[ignore = "requires explicitly configured rooted TV and trusted SSH identity via private stdin"]
async fn capture_from_trusted_tv() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
    let mut lines = input.lines();
    let config = CaptureConfig {
        endpoint: lines.next().unwrap().parse().unwrap(),
        username: lines.next().unwrap().into(),
        agent_socket: lines.next().unwrap().into(),
        public_key: lines.next().unwrap().into(),
    };
    let mut ssh = CaptureSession::connect(&config)
        .await
        .expect("trusted SSH connection");
    let image = ssh.capture().await.expect("fresh compositor image");
    assert!(matches!(image.mime_type, "image/jpeg" | "image/png"));
    assert!(!image.bytes.is_empty());
    ssh.close().await.expect("close SSH session");
}
