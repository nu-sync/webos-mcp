use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::{ServerConfig, pki_types::PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use webos_mcp::transport;

async fn simulated_tv(
    register: bool,
    answer_probe: bool,
) -> (
    std::net::SocketAddr,
    [u8; 32],
    tokio::task::JoinHandle<bool>,
) {
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["example.invalid".into()]).unwrap();
    let fingerprint: [u8; 32] = Sha256::digest(cert.der().as_ref()).into();
    let private_key = PrivatePkcs8KeyDer::from(signing_key.serialize_der());
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], private_key.into())
            .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        match TlsAcceptor::from(Arc::new(config)).accept(stream).await {
            Ok(tls) => {
                let Ok(mut socket) = tokio_tungstenite::accept_async(tls).await else {
                    return false;
                };
                if register {
                    let Some(Ok(request)) = socket.next().await else {
                        return false;
                    };
                    let Ok(value) =
                        serde_json::from_str::<serde_json::Value>(request.to_text().unwrap_or(""))
                    else {
                        return false;
                    };
                    if value["type"] != "register"
                        || value["payload"]["client-key"] != "synthetic-key"
                    {
                        return false;
                    }
                    let response = r#"{"id":"register_0","type":"registered","payload":{"client-key":"synthetic-key"}}"#;
                    return socket
                        .send(tokio_tungstenite::tungstenite::Message::Text(
                            response.into(),
                        ))
                        .await
                        .is_ok();
                }
                if answer_probe {
                    let Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(data))) =
                        socket.next().await
                    else {
                        return false;
                    };
                    if socket
                        .send(tokio_tungstenite::tungstenite::Message::Pong(
                            vec![0; 8].into(),
                        ))
                        .await
                        .is_err()
                    {
                        return false;
                    }
                    return socket
                        .send(tokio_tungstenite::tungstenite::Message::Pong(data))
                        .await
                        .is_ok();
                }
                true
            }
            Err(_) => false,
        }
    });
    (endpoint, fingerprint, server)
}

#[tokio::test]
async fn verified_certificate_allows_websocket_connection() {
    let (endpoint, pin, server) = simulated_tv(false, false).await;
    let _socket = transport::connect(endpoint, pin)
        .await
        .expect("verified connection");
    assert!(server.await.unwrap());
}

#[tokio::test]
async fn changed_certificate_stops_before_websocket_or_registration() {
    let (endpoint, mut pin, server) = simulated_tv(false, false).await;
    pin[0] ^= 1;
    assert!(matches!(
        transport::connect(endpoint, pin).await,
        Err("TV TLS authentication failed")
    ));
    assert!(!server.await.unwrap());
}

#[tokio::test]
async fn verified_connection_registers_with_existing_key() {
    let (endpoint, pin, server) = simulated_tv(true, false).await;
    let mut socket = transport::connect(endpoint, pin).await.unwrap();
    assert_eq!(
        transport::register(&mut socket, Some("synthetic-key"))
            .await
            .unwrap(),
        "synthetic-key"
    );
    assert!(server.await.unwrap());
}

#[tokio::test]
async fn preflight_detects_a_closed_socket_before_any_action() {
    let (endpoint, pin, server) = simulated_tv(false, false).await;
    let mut socket = transport::connect(endpoint, pin).await.unwrap();
    assert!(server.await.unwrap());
    assert!(matches!(
        socket.probe().await,
        Err("TV connection is stale; reconnect and observe again")
    ));
}

#[tokio::test]
async fn preflight_succeeds_when_peer_replies_to_ping() {
    let (endpoint, pin, server) = simulated_tv(false, true).await;
    let mut socket = transport::connect(endpoint, pin).await.unwrap();
    assert!(socket.probe().await.is_ok());
    assert!(server.await.unwrap());
}
