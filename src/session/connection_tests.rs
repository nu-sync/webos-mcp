use super::*;

#[tokio::test]
async fn refused_control_connection_can_be_closed_and_retried() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    drop(listener);
    let mut device = LiveDevice {
        remote: None,
        input: None,
        capture: None,
        cleanup_unconfirmed: false,
    };
    for _ in 0..2 {
        assert!(device.connect_control(endpoint, [0; 32]).await.is_err());
        assert!(!device.cleanup_unconfirmed);
        assert!(device.close().await.is_ok());
    }
}

#[tokio::test]
async fn cancelled_control_handshake_does_not_claim_confirmed_cleanup() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let mut device = LiveDevice {
        remote: None,
        input: None,
        capture: None,
        cleanup_unconfirmed: false,
    };
    assert!(
        tokio::time::timeout(
            Duration::from_millis(30),
            device.connect_control(endpoint, [0; 32]),
        )
        .await
        .is_err()
    );
    assert!(device.cleanup_unconfirmed);
    assert!(device.close().await.is_err());
}
