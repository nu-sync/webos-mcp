//! An explicitly pinned TLS connection to one configured endpoint.

use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::time::Instant;

use futures_util::{SinkExt, StreamExt};
use rustls::{
    ClientConfig, DigitallySignedStruct, Error, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::{TlsConnector, client::TlsStream};
use tokio_tungstenite::{
    WebSocketStream, client_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

use crate::error::{TvError, outcome};
use crate::metrics::{Metrics, Operation, Stage};
use crate::pairing::{Registration, parse_registration, registration_request};

struct PinnedCertificate {
    digest: [u8; 32],
    algorithms: WebPkiSupportedAlgorithms,
}

pub struct AuthenticatedSocket {
    socket: WebSocketStream<TlsStream<TcpStream>>,
    pings: u64,
    /// Reused for the input socket so it can resume this TLS session.
    tls: Arc<ClientConfig>,
}

impl std::fmt::Debug for PinnedCertificate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PinnedCertificate([redacted])")
    }
}

impl ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let actual: [u8; 32] = Sha256::digest(cert.as_ref()).into();
        if actual != self.digest {
            return Err(Error::General(
                "TV certificate does not match the verified fingerprint".into(),
            ));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(message, cert, signature, &self.algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, signature, &self.algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

const CONNECT_BUDGET: Duration = Duration::from_secs(10);
const STALE: &str = "TV connection is stale; reconnect and observe again";
const CLEANUP: &str = "TV connection cleanup could not be confirmed";

/// A TLS client that accepts only the certificate with this fingerprint.
/// Connections sharing one configuration may resume each other's sessions,
/// which the pinned certificate authenticated when they were established.
fn pinned_config(fingerprint: [u8; 32]) -> Result<Arc<ClientConfig>, TvError> {
    let provider = rustls::crypto::ring::default_provider();
    let verifier = PinnedCertificate {
        digest: fingerprint,
        algorithms: provider.signature_verification_algorithms,
    };
    Ok(Arc::new(
        ClientConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()
            .map_err(|_| "TLS configuration failed")?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth(),
    ))
}

/// Connect to exactly one endpoint using a fingerprint verified independently by the user.
/// No registration credential is sent by this function.
pub async fn connect(
    endpoint: SocketAddr,
    fingerprint: [u8; 32],
    metrics: &Metrics,
) -> Result<AuthenticatedSocket, TvError> {
    connect_path(
        endpoint,
        pinned_config(fingerprint)?,
        "/",
        metrics,
        CONNECT_BUDGET,
    )
    .await
}

async fn step<T>(
    metrics: &Metrics,
    stage: Stage,
    deadline: Instant,
    timed_out: &'static str,
    work: impl Future<Output = Result<T, &'static str>>,
) -> Result<T, TvError> {
    let measurement = metrics.stage(Operation::Connect, stage);
    let result = match tokio::time::timeout_at(deadline, work).await {
        Ok(finished) => finished.map_err(TvError::failure),
        Err(_) => Err(TvError::timeout(timed_out)),
    };
    measurement.finish(outcome(&result));
    result
}

/// Each step shares one overall budget; a timeout names the step that stalled.
/// Nothing is sent to the TV before all three steps succeed.
async fn connect_path(
    endpoint: SocketAddr,
    tls: Arc<ClientConfig>,
    path: &str,
    metrics: &Metrics,
    budget: Duration,
) -> Result<AuthenticatedSocket, TvError> {
    let name = ServerName::try_from("webos-tv.invalid").map_err(|_| "TLS configuration failed")?;
    let deadline = Instant::now() + budget;
    let tcp = step(
        metrics,
        Stage::TcpConnect,
        deadline,
        "TV connection timed out before the TV accepted it; nothing was sent",
        async {
            let tcp = TcpStream::connect(endpoint)
                .await
                .map_err(|_| "Could not connect to the TV endpoint; nothing was sent")?;
            // Button and ping frames are tiny; never hold one back to coalesce.
            tcp.set_nodelay(true)
                .map_err(|_| "Could not connect to the TV endpoint; nothing was sent")?;
            Ok(tcp)
        },
    )
    .await?;
    let stream = step(
        metrics,
        Stage::TlsHandshake,
        deadline,
        "TV connection timed out during the TLS handshake; nothing was sent",
        async {
            TlsConnector::from(tls.clone())
                .connect(name, tcp)
                .await
                .map_err(|_| "TV TLS authentication failed")
        },
    )
    .await?;
    let limits = WebSocketConfig::default()
        .max_message_size(Some(32 * 1024))
        .max_frame_size(Some(32 * 1024));
    let uri = format!("wss://webos-tv.invalid{path}");
    let (socket, _) = step(
        metrics,
        Stage::WebsocketHandshake,
        deadline,
        "TV connection timed out during the websocket handshake; nothing was sent",
        async {
            client_async_with_config(uri, stream, Some(limits))
                .await
                .map_err(|_| "TV websocket handshake failed; nothing was sent")
        },
    )
    .await?;
    Ok(AuthenticatedSocket {
        socket,
        pings: 0,
        tls,
    })
}

impl AuthenticatedSocket {
    /// Check the socket without sending a TV action. A failed check must be
    /// followed by reconnecting; the caller must not retry a button blindly.
    pub async fn probe(&mut self) -> Result<(), TvError> {
        let check = async {
            self.pings = self.pings.checked_add(1).ok_or(STALE)?;
            let payload = self.pings.to_be_bytes();
            self.socket
                .send(Message::Ping(payload.to_vec().into()))
                .await
                .map_err(|_| STALE)?;
            loop {
                match self.socket.next().await {
                    Some(Ok(Message::Pong(data))) if data.as_ref() == payload => return Ok(()),
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
                    _ => return Err(TvError::failure(STALE)),
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(3), check)
            .await
            .map_err(|_| TvError::timeout(STALE))?
    }

    pub async fn close(mut self) -> Result<(), TvError> {
        tokio::time::timeout(Duration::from_secs(3), async {
            self.socket.close(None).await.map_err(|_| CLEANUP)?;
            while let Some(message) = self.socket.next().await {
                match message {
                    Ok(Message::Close(_)) => return Ok(()),
                    Err(tokio_tungstenite::tungstenite::Error::ConnectionClosed) => return Ok(()),
                    Err(_) => return Err(TvError::failure(CLEANUP)),
                    _ => {}
                }
            }
            Err(TvError::failure(CLEANUP))
        })
        .await
        .map_err(|_| TvError::timeout(CLEANUP))?
    }

    async fn request(
        &mut self,
        uri: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, TvError> {
        let work = async {
            let request =
                serde_json::json!({"id":"request_0","type":"request","uri":uri,"payload":payload});
            self.socket
                .send(Message::Text(request.to_string().into()))
                .await
                .map_err(|_| "TV request failed")?;
            loop {
                let message = self
                    .socket
                    .next()
                    .await
                    .ok_or("TV connection closed")?
                    .map_err(|_| "TV response failed")?;
                match message {
                    Message::Text(text) => {
                        let value: serde_json::Value =
                            serde_json::from_str(&text).map_err(|_| "Invalid TV response")?;
                        if value.get("id").and_then(|v| v.as_str()) != Some("request_0") {
                            return Err(TvError::failure("Unexpected TV response"));
                        }
                        if value.get("type").and_then(|v| v.as_str()) != Some("response") {
                            return Err(TvError::failure("TV rejected request"));
                        }
                        return Ok(value.get("payload").cloned().ok_or("Invalid TV response")?);
                    }
                    Message::Ping(_) | Message::Pong(_) => continue,
                    _ => return Err(TvError::failure("Unexpected TV response")),
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), work)
            .await
            .map_err(|_| TvError::timeout("TV request timed out"))?
    }

    pub async fn input_socket(
        &mut self,
        endpoint: SocketAddr,
        metrics: &Metrics,
    ) -> Result<AuthenticatedSocket, TvError> {
        let response = self
            .request(
                "ssap://com.webos.service.networkinput/getPointerInputSocket",
                serde_json::json!({}),
            )
            .await?;
        let raw = response
            .get("socketPath")
            .and_then(|v| v.as_str())
            .ok_or("Invalid TV input endpoint")?;
        let url = url::Url::parse(raw).map_err(|_| "Invalid TV input endpoint")?;
        if !matches!(url.scheme(), "ws" | "wss")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || (url.host_str() != Some("webos-tv.invalid")
                && url
                    .host_str()
                    .and_then(|host| host.parse::<std::net::IpAddr>().ok())
                    != Some(endpoint.ip()))
        {
            return Err(TvError::failure(
                "TV input endpoint does not match the selected TV",
            ));
        }
        let mut path = url.path().to_owned();
        if let Some(query) = url.query() {
            path.push('?');
            path.push_str(query);
        }
        connect_path(endpoint, self.tls.clone(), &path, metrics, CONNECT_BUDGET).await
    }

    pub async fn send_button(&mut self, name: &str) -> Result<(), TvError> {
        const UNCERTAIN: &str = "Button delivery is uncertain";
        let message = format!("type:button\nname:{name}\n\n");
        tokio::time::timeout(
            Duration::from_secs(5),
            self.socket.send(Message::Text(message.into())),
        )
        .await
        .map_err(|_| TvError::timeout(UNCERTAIN))?
        .map_err(|_| TvError::failure(UNCERTAIN))
    }

    pub async fn launch(&mut self, app_id: &str) -> Result<(), TvError> {
        let payload = self
            .request(
                "ssap://system.launcher/launch",
                serde_json::json!({"id":app_id}),
            )
            .await?;
        if payload.get("returnValue").and_then(|v| v.as_bool()) == Some(true) {
            Ok(())
        } else {
            Err(TvError::failure("TV did not acknowledge launch"))
        }
    }
}

/// Register only on a connection already authenticated by `connect`.
pub async fn register(
    socket: &mut AuthenticatedSocket,
    key: Option<&str>,
) -> Result<String, TvError> {
    let exchange = async {
        socket
            .socket
            .send(Message::Text(registration_request(key).into()))
            .await
            .map_err(|_| "TV registration request failed")?;
        loop {
            let message = socket
                .socket
                .next()
                .await
                .ok_or("TV closed registration")?
                .map_err(|_| "TV registration response failed")?;
            match message {
                Message::Text(text) => match parse_registration(text.as_bytes())? {
                    Registration::Prompt => continue,
                    Registration::Registered(key) => return Ok(key),
                    Registration::Rejected => {
                        return Err(TvError::failure("TV rejected registration"));
                    }
                },
                Message::Ping(_) | Message::Pong(_) => continue,
                _ => return Err(TvError::failure("Unexpected TV registration response")),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(60), exchange)
        .await
        .map_err(|_| TvError::timeout("TV registration timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertifiedKey, generate_simple_self_signed};
    use rustls::{ServerConfig, pki_types::PrivatePkcs8KeyDer};
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    const BUDGET: Duration = Duration::from_millis(200);

    fn connect_stages(metrics: &Metrics) -> [(u64, u64, u64); 3] {
        let snapshot = metrics.snapshot();
        let stages = &snapshot.stages[&Operation::Connect];
        [
            Stage::TcpConnect,
            Stage::TlsHandshake,
            Stage::WebsocketHandshake,
        ]
        .map(|stage| {
            let outcomes = &stages[&stage].outcomes;
            (outcomes.success, outcomes.failure, outcomes.timeout)
        })
    }

    #[tokio::test]
    async fn silent_peer_times_out_in_the_tls_step() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let metrics = Metrics::new(true);
        let error = connect_path(
            endpoint,
            pinned_config([0; 32]).unwrap(),
            "/",
            &metrics,
            BUDGET,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            error,
            "TV connection timed out during the TLS handshake; nothing was sent"
        );
        assert_eq!(connect_stages(&metrics), [(1, 0, 0), (0, 0, 1), (0, 0, 0)]);
        drop(listener);
    }

    #[tokio::test]
    async fn sockets_sharing_a_pinned_configuration_resume_and_disable_nagle() {
        use rustls::HandshakeKind;

        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["example.invalid".into()]).unwrap();
        let fingerprint: [u8; 32] = Sha256::digest(cert.der().as_ref()).into();
        let config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    vec![cert.der().clone()],
                    PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
                )
                .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let server = tokio::spawn(async move {
            let mut open = Vec::new();
            for _ in 0..3 {
                let (stream, _) = listener.accept().await.unwrap();
                let tls = acceptor.accept(stream).await.unwrap();
                open.push(tokio_tungstenite::accept_async(tls).await.unwrap());
            }
            for mut socket in open {
                while socket.next().await.is_some() {}
            }
        });
        let metrics = Metrics::new(false);
        let budget = Duration::from_secs(5);
        let kind =
            |socket: &AuthenticatedSocket| socket.socket.get_ref().get_ref().1.handshake_kind();
        let shared = pinned_config(fingerprint).unwrap();
        let control = connect_path(endpoint, shared.clone(), "/", &metrics, budget)
            .await
            .unwrap();
        let input = connect_path(endpoint, control.tls.clone(), "/input", &metrics, budget)
            .await
            .unwrap();
        let separate = connect_path(
            endpoint,
            pinned_config(fingerprint).unwrap(),
            "/",
            &metrics,
            budget,
        )
        .await
        .unwrap();
        assert_eq!(kind(&control), Some(HandshakeKind::Full));
        assert_eq!(kind(&input), Some(HandshakeKind::Resumed));
        assert_eq!(kind(&separate), Some(HandshakeKind::Full));
        assert!(input.socket.get_ref().get_ref().0.nodelay().unwrap());
        drop((control, input, separate));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn peer_that_stalls_after_tls_times_out_in_the_websocket_step() {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["example.invalid".into()]).unwrap();
        let fingerprint: [u8; 32] = Sha256::digest(cert.der().as_ref()).into();
        let config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    vec![cert.der().clone()],
                    PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
                )
                .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let (release, held) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _tls = TlsAcceptor::from(Arc::new(config))
                .accept(stream)
                .await
                .unwrap();
            let _ = held.await;
        });
        let metrics = Metrics::new(true);
        let error = connect_path(
            endpoint,
            pinned_config(fingerprint).unwrap(),
            "/",
            &metrics,
            BUDGET,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            error,
            "TV connection timed out during the websocket handshake; nothing was sent"
        );
        assert_eq!(connect_stages(&metrics), [(1, 0, 0), (1, 0, 0), (0, 0, 1)]);
        drop(release);
        server.await.unwrap();
    }
}
