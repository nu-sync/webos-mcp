use super::*;

#[tokio::test]
async fn refused_control_connection_can_be_closed_and_retried() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    drop(listener);
    let metrics = Metrics::new(true);
    let mut device = LiveDevice::idle();
    for _ in 0..2 {
        assert_eq!(
            device
                .connect_control(endpoint, [0; 32], &metrics)
                .await
                .unwrap_err(),
            "Could not connect to the TV endpoint; nothing was sent"
        );
        assert!(!device.cleanup_unconfirmed);
        assert!(device.close().await.is_ok());
    }
    let stages = &metrics.snapshot().stages[&Operation::Connect];
    assert_eq!(stages[&Stage::TcpConnect].outcomes.failure, 2);
    assert_eq!(stages[&Stage::TlsHandshake].count, 0);
}

#[tokio::test]
async fn cancelled_control_handshake_does_not_claim_confirmed_cleanup() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let metrics = Metrics::new(true);
    let mut device = LiveDevice::idle();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(30),
            device.connect_control(endpoint, [0; 32], &metrics),
        )
        .await
        .is_err()
    );
    assert!(device.cleanup_unconfirmed);
    assert!(device.close().await.is_err());
    let stages = &metrics.snapshot().stages[&Operation::Connect];
    assert_eq!(stages[&Stage::TcpConnect].outcomes.success, 1);
    assert_eq!(stages[&Stage::TlsHandshake].outcomes.cancelled, 1);
}

#[tokio::test]
async fn unconfirmed_cleanup_names_the_restart_and_is_not_a_timeout() {
    let mut device = LiveDevice::idle();
    device.cleanup_unconfirmed = true;
    let unconfirmed = device.close().await.unwrap_err();
    assert_eq!(unconfirmed.outcome(), Outcome::Failure);
    assert!(unconfirmed.message().contains("restart the MCP server"));
    assert_eq!(device.probe().await.unwrap_err(), "TV input is unavailable");
}
