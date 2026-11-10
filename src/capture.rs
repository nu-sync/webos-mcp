//! Optional rooted compositor capture over an existing trusted SSH identity.
//! The TV's own encoding (JPEG by default, PNG when lossless) is returned as is.
use crate::error::{TvError, outcome};
use crate::metrics::{Metrics, Operation, Stage};
use russh::{
    ChannelMsg, client,
    keys::{PublicKey, PublicKeyOrCertificate, agent::client::AgentClient},
};
use sha2::{Digest, Sha256};
use std::{future::Future, io::Cursor, net::SocketAddr, sync::Arc, time::Duration};

const MAX_PNG: usize = 16 * 1024 * 1024;
const MAX_DECODED: usize = 64 * 1024 * 1024;
const CAPTURE_PNG: &str = r#"umask 077
 d=$(mktemp -d /tmp/webos-mcp.XXXXXX) || exit 1
 trap 'rm -rf "$d"' EXIT HUP INT TERM
 luna-send -w 5000 -n 1 -f luna://com.webos.surfacemanager/captureCompositorOutput "{\"output\":\"$d/screen.png\",\"format\":\"PNG\"}" > "$d/response" || exit 1
 grep -Eq '"returnValue"[[:space:]]*:[[:space:]]*true' "$d/response" || exit 1
 test -s "$d/screen.png" || exit 1
 cat "$d/screen.png" || exit 1
 rm -rf "$d" || exit 1
 trap - EXIT HUP INT TERM"#;
const CAPTURE: &str = r#"umask 077
 d=$(mktemp -d /tmp/webos-mcp.XXXXXX) || exit 1
 trap 'rm -rf "$d"' EXIT HUP INT TERM
 luna-send -w 5000 -n 1 -f luna://com.webos.surfacemanager/captureCompositorOutput "{\"output\":\"$d/screen.png\",\"format\":\"JPG\"}" > "$d/response" || exit 1
 if grep -q 'ERR_INVALID_FORMAT' "$d/response"; then
   luna-send -w 5000 -n 1 -f luna://com.webos.surfacemanager/captureCompositorOutput "{\"output\":\"$d/screen.png\",\"format\":\"PNG\"}" > "$d/response" || exit 1
 fi
 grep -Eq '"returnValue"[[:space:]]*:[[:space:]]*true' "$d/response" || exit 1
 test -s "$d/screen.png" || exit 1
 cat "$d/screen.png" || exit 1
 rm -rf "$d" || exit 1
 trap - EXIT HUP INT TERM"#;

#[derive(Clone)]
pub struct CaptureConfig {
    pub endpoint: SocketAddr,
    pub username: String,
    /// Preferred SSH agent socket or pipe. Empty means the `SSH_AUTH_SOCK` of
    /// the server's environment, which is also the fallback when this path no
    /// longer accepts connections.
    pub agent_socket: String,
    pub public_key: String,
}
struct HostVerifier {
    host: String,
    port: u16,
}
impl client::Handler for HostVerifier {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        if key.certificate().is_some() {
            return Ok(false);
        }
        Ok(
            russh::keys::check_known_hosts(&self.host, self.port, &key.public_key())
                .unwrap_or(false),
        )
    }
}
pub struct CaptureSession(client::Handle<HostVerifier>);
pub struct CapturedImage {
    /// The image exactly as the TV encoded it.
    pub bytes: Vec<u8>,
    pub mime_type: &'static str,
    pub all_black: bool,
    pub fingerprint: [u8; 32],
    pub width: u32,
    pub height: u32,
    pub recovered: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureError {
    Connection,
    ChannelOpen,
    Request,
    Transfer,
    Timeout,
    TransferLimit,
    InvalidImage,
    DecodedLimit,
    CleanupUnconfirmed,
    Closed,
}

impl CaptureError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Connection => "capture_connection_failed",
            Self::ChannelOpen => "capture_channel_failed",
            Self::Request => "capture_request_failed",
            Self::Transfer => "capture_transfer_failed",
            Self::Timeout => "capture_timeout",
            Self::TransferLimit => "capture_transfer_limit",
            Self::InvalidImage => "capture_invalid_image",
            Self::DecodedLimit => "capture_decoded_limit",
            Self::CleanupUnconfirmed => "capture_cleanup_unconfirmed",
            Self::Closed => "capture_closed",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Connection => "Trusted capture connection could not be established",
            Self::ChannelOpen => "Capture channel could not be opened",
            Self::Request => "Capture request failed; remote cleanup is unconfirmed",
            Self::Transfer => "Capture transfer failed; remote cleanup is unconfirmed",
            Self::Timeout => "Capture timed out; remote cleanup is unconfirmed",
            Self::TransferLimit => {
                "Capture exceeded the transfer limit; remote cleanup is unconfirmed"
            }
            Self::InvalidImage => "TV returned an invalid image",
            Self::DecodedLimit => "TV image exceeds the decoded size limit",
            Self::CleanupUnconfirmed => {
                "Capture cleanup is unconfirmed; further capture is blocked"
            }
            Self::Closed => "Capture connection is closed",
        }
    }

    pub fn cleanup_unconfirmed(self) -> bool {
        matches!(
            self,
            Self::Request
                | Self::Transfer
                | Self::Timeout
                | Self::TransferLimit
                | Self::CleanupUnconfirmed
        )
    }

    pub fn retryable(self) -> bool {
        matches!(self, Self::ChannelOpen)
    }
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}
impl std::error::Error for CaptureError {}
impl From<CaptureError> for TvError {
    fn from(error: CaptureError) -> Self {
        match error {
            CaptureError::Timeout => Self::timeout(error.message()),
            _ => Self::failure(error.message()),
        }
    }
}

/// Agent endpoints to try in order: the configured one, then the environment's.
/// Either may be absent; the configured public key still selects the identity.
fn agent_candidates(configured: &str, environment: Option<String>) -> Vec<String> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    let fallback = Some(r"\\.\pipe\openssh-ssh-agent".to_owned());
    #[cfg(not(windows))]
    let fallback = None;
    for candidate in [Some(configured.to_owned()), environment, fallback]
        .into_iter()
        .flatten()
    {
        let candidate = candidate.trim().to_owned();
        if !candidate.is_empty() && !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

async fn connect_agent<S>(
    candidates: Vec<String>,
    connect: impl AsyncFn(String) -> Option<S>,
) -> Result<S, CaptureError> {
    for candidate in candidates {
        if let Some(agent) = connect(candidate).await {
            return Ok(agent);
        }
    }
    Err(CaptureError::Connection)
}
impl CaptureSession {
    pub async fn connect(config: &CaptureConfig) -> Result<Self, &'static str> {
        Self::connect_trusted(config)
            .await
            .map_err(|error| error.message())
    }

    async fn connect_trusted(config: &CaptureConfig) -> Result<Self, CaptureError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let key =
            PublicKey::from_openssh(&config.public_key).map_err(|_| CaptureError::Connection)?;
        let verifier = HostVerifier {
            host: config.endpoint.ip().to_string(),
            port: config.endpoint.port(),
        };
        let ssh_config = client::Config {
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 3,
            ..Default::default()
        };
        let mut session = tokio::time::timeout_at(
            deadline,
            client::connect(Arc::new(ssh_config), config.endpoint, verifier),
        )
        .await
        .map_err(|_| CaptureError::Connection)?
        .map_err(|_| CaptureError::Connection)?;
        let authenticate = async {
            let candidates =
                agent_candidates(&config.agent_socket, std::env::var("SSH_AUTH_SOCK").ok());
            #[cfg(unix)]
            let mut agent = connect_agent(candidates, async |path: String| {
                AgentClient::connect_uds(&path).await.ok()
            })
            .await?;
            #[cfg(windows)]
            let mut agent = connect_agent(candidates, async |path: String| {
                AgentClient::connect_named_pipe(&path).await.ok()
            })
            .await?;
            let hash = session
                .best_supported_rsa_hash()
                .await
                .map_err(|_| CaptureError::Connection)?
                .flatten();
            let result = session
                .authenticate_publickey_with(&config.username, key, hash, &mut agent)
                .await
                .map_err(|_| CaptureError::Connection)?;
            if result.success() {
                Ok(())
            } else {
                Err(CaptureError::Connection)
            }
        };
        let authenticated = matches!(
            tokio::time::timeout_at(deadline, authenticate).await,
            Ok(Ok(()))
        );
        finish_authentication(Self(session), authenticated).await
    }

    pub async fn capture(&mut self) -> Result<CapturedImage, CaptureError> {
        self.capture_inner(None).await
    }

    pub async fn capture_with_metrics(
        &mut self,
        operation: Operation,
        metrics: &Metrics,
    ) -> Result<CapturedImage, CaptureError> {
        self.capture_inner(Some((metrics, operation))).await
    }

    async fn capture_inner(
        &mut self,
        metrics: Option<(&Metrics, Operation)>,
    ) -> Result<CapturedImage, CaptureError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let mut channel = capture_stage(metrics, Stage::CaptureRequest, deadline, async {
            let channel = self
                .0
                .channel_open_session()
                .await
                .map_err(|_| CaptureError::ChannelOpen)?;
            let command = if std::env::var_os("WEBOS_MCP_LOSSLESS").as_deref()
                == Some(std::ffi::OsStr::new("1"))
            {
                CAPTURE_PNG
            } else {
                CAPTURE
            };
            channel
                .exec(true, command)
                .await
                .map_err(|_| CaptureError::Request)?;
            Ok(channel)
        })
        .await?;
        let output = capture_stage(metrics, Stage::ImageTransfer, deadline, async {
            let mut output = Vec::new();
            let mut exit = None;
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } => {
                        if output.len().saturating_add(data.len()) > MAX_PNG {
                            return Err(CaptureError::TransferLimit);
                        }
                        output.extend_from_slice(&data);
                    }
                    ChannelMsg::ExitStatus { exit_status } => exit = Some(exit_status),
                    _ => {}
                }
            }
            if exit != Some(0) {
                return Err(CaptureError::Transfer);
            }
            Ok(output)
        })
        .await?;
        capture_stage(metrics, Stage::ImageValidation, deadline, async {
            // Decoding is CPU-bound; keep it off the async workers.
            tokio::task::spawn_blocking(move || decode_image(output))
                .await
                .map_err(|_| CaptureError::InvalidImage)?
        })
        .await
    }
    pub async fn close(self) -> Result<(), &'static str> {
        let session = self.0;
        confirm_ssh_close(Duration::from_secs(5), async {
            session
                .disconnect(russh::Disconnect::ByApplication, "", "")
                .await
                .map_err(|_| "SSH cleanup could not be confirmed")?;
            match session.await {
                Ok(()) | Err(russh::Error::Disconnect) => Ok(()),
                Err(_) => Err("SSH cleanup could not be confirmed"),
            }
        })
        .await
    }
}

async fn measure_capture_stage<T, F>(
    metrics: Option<(&Metrics, Operation)>,
    stage: Stage,
    work: F,
) -> Result<T, CaptureError>
where
    F: Future<Output = Result<T, CaptureError>>,
{
    let measurement = metrics.map(|(metrics, operation)| metrics.stage(operation, stage));
    let result = work.await;
    if let Some(measurement) = measurement {
        measurement.finish(outcome(&result));
    }
    result
}

async fn capture_stage<T, F>(
    metrics: Option<(&Metrics, Operation)>,
    stage: Stage,
    deadline: tokio::time::Instant,
    work: F,
) -> Result<T, CaptureError>
where
    F: Future<Output = Result<T, CaptureError>>,
{
    measure_capture_stage(metrics, stage, async {
        tokio::time::timeout_at(deadline, work)
            .await
            .map_err(|_| CaptureError::Timeout)?
    })
    .await
}

async fn confirm_ssh_close<F>(timeout: Duration, close: F) -> Result<(), &'static str>
where
    F: std::future::Future<Output = Result<(), &'static str>>,
{
    tokio::time::timeout(timeout, close)
        .await
        .map_err(|_| "SSH cleanup could not be confirmed")?
}
/// What the session needs to know about decoded pixels, whatever the encoding.
struct Pixels<'a> {
    width: u32,
    height: u32,
    color: png::ColorType,
    depth: png::BitDepth,
    samples: &'a [u8],
}

impl Pixels<'_> {
    fn describe(self, bytes: Vec<u8>, mime_type: &'static str) -> CapturedImage {
        let bytes_per_sample = match self.depth {
            png::BitDepth::Sixteen => 2,
            _ => 1,
        };
        let channels = self.color.samples();
        let color_channels = if channels == 2 || channels == 4 {
            channels - 1
        } else {
            channels
        };
        let all_black = self
            .samples
            .chunks_exact(channels * bytes_per_sample)
            .all(|pixel| {
                pixel[..color_channels * bytes_per_sample]
                    .iter()
                    .all(|&component| component == 0)
            });
        let mut hash = Sha256::new();
        hash.update(self.width.to_be_bytes());
        hash.update(self.height.to_be_bytes());
        hash.update([self.color as u8, self.depth as u8]);
        hash.update(self.samples);
        CapturedImage {
            bytes,
            mime_type,
            all_black,
            fingerprint: hash.finalize().into(),
            width: self.width,
            height: self.height,
            recovered: false,
        }
    }
}

/// Fully decode the TV's image to prove it is complete, then return the
/// original bytes with a fingerprint of the decoded pixels.
fn decode_image(bytes: Vec<u8>) -> Result<CapturedImage, CaptureError> {
    if bytes.starts_with(&[0xff, 0xd8]) {
        return decode_jpeg(bytes);
    }
    let mut decoder = png::Decoder::new(Cursor::new(&bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder
        .read_info()
        .map_err(|_| CaptureError::InvalidImage)?;
    let size = reader
        .output_buffer_size()
        .ok_or(CaptureError::DecodedLimit)?;
    if size > MAX_DECODED {
        return Err(CaptureError::DecodedLimit);
    }
    let mut pixels = vec![0; size];
    let frame = reader
        .next_frame(&mut pixels)
        .map_err(|_| CaptureError::InvalidImage)?;
    reader.finish().map_err(|_| CaptureError::InvalidImage)?;
    if frame.color_type == png::ColorType::Indexed
        || !matches!(
            frame.bit_depth,
            png::BitDepth::Eight | png::BitDepth::Sixteen
        )
    {
        return Err(CaptureError::InvalidImage);
    }
    let decoded = Pixels {
        width: frame.width,
        height: frame.height,
        color: frame.color_type,
        depth: frame.bit_depth,
        samples: &pixels[..frame.buffer_size()],
    };
    Ok(decoded.describe(bytes, "image/png"))
}

fn decode_jpeg(bytes: Vec<u8>) -> Result<CapturedImage, CaptureError> {
    use image::ImageDecoder;

    if !bytes.ends_with(&[0xff, 0xd9]) {
        return Err(CaptureError::InvalidImage);
    }
    let decoder = image::codecs::jpeg::JpegDecoder::new(Cursor::new(&bytes))
        .map_err(|_| CaptureError::InvalidImage)?;
    let (width, height) = decoder.dimensions();
    let size = usize::try_from(decoder.total_bytes()).map_err(|_| CaptureError::DecodedLimit)?;
    if size > MAX_DECODED {
        return Err(CaptureError::DecodedLimit);
    }
    let color = match decoder.color_type() {
        image::ColorType::L8 => png::ColorType::Grayscale,
        image::ColorType::Rgb8 => png::ColorType::Rgb,
        _ => return Err(CaptureError::InvalidImage),
    };
    let mut pixels = vec![0; size];
    decoder
        .read_image(&mut pixels)
        .map_err(|_| CaptureError::InvalidImage)?;
    let decoded = Pixels {
        width,
        height,
        color,
        depth: png::BitDepth::Eight,
        samples: &pixels,
    };
    Ok(decoded.describe(bytes, "image/jpeg"))
}

trait CaptureConnection: Sized {
    fn connect(config: &CaptureConfig) -> impl Future<Output = Result<Self, CaptureError>> + Send;
    fn capture(&mut self) -> impl Future<Output = Result<CapturedImage, CaptureError>> + Send;
    fn capture_with_metrics(
        &mut self,
        _operation: Operation,
        _metrics: &Metrics,
    ) -> impl Future<Output = Result<CapturedImage, CaptureError>> + Send {
        self.capture()
    }
    fn close(self) -> impl Future<Output = Result<(), CaptureError>> + Send;
}

impl CaptureConnection for CaptureSession {
    async fn connect(config: &CaptureConfig) -> Result<Self, CaptureError> {
        CaptureSession::connect_trusted(config).await
    }
    async fn capture(&mut self) -> Result<CapturedImage, CaptureError> {
        CaptureSession::capture(self).await
    }
    async fn capture_with_metrics(
        &mut self,
        operation: Operation,
        metrics: &Metrics,
    ) -> Result<CapturedImage, CaptureError> {
        CaptureSession::capture_with_metrics(self, operation, metrics).await
    }
    async fn close(self) -> Result<(), CaptureError> {
        CaptureSession::close(self)
            .await
            .map_err(|_| CaptureError::CleanupUnconfirmed)
    }
}

async fn finish_authentication<C: CaptureConnection>(
    connection: C,
    authenticated: bool,
) -> Result<C, CaptureError> {
    if authenticated {
        return Ok(connection);
    }
    connection.close().await?;
    Err(CaptureError::Connection)
}

enum CaptureState<C> {
    Disconnected,
    Connected(C),
    CleanupUnconfirmed,
    Closed,
}

struct Recovery<C> {
    config: CaptureConfig,
    state: CaptureState<C>,
}

impl<C: CaptureConnection> Recovery<C> {
    async fn connect(config: CaptureConfig) -> Result<Self, CaptureError> {
        let connection = C::connect(&config).await?;
        Ok(Self {
            config,
            state: CaptureState::Connected(connection),
        })
    }

    async fn capture(&mut self) -> Result<CapturedImage, CaptureError> {
        self.capture_inner(None).await
    }

    async fn capture_inner(
        &mut self,
        metrics: Option<(&Metrics, Operation)>,
    ) -> Result<CapturedImage, CaptureError> {
        // A call may replace one failed connection, but never replay an action.
        let mut recovered = false;
        for attempt in 0..2 {
            if matches!(self.state, CaptureState::Disconnected) {
                self.state = CaptureState::CleanupUnconfirmed;
                match measure_capture_stage(
                    metrics,
                    Stage::ConnectionSetup,
                    C::connect(&self.config),
                )
                .await
                {
                    Ok(connection) => {
                        self.state = CaptureState::Connected(connection);
                        recovered = true;
                    }
                    Err(CaptureError::Connection) => {
                        self.state = CaptureState::Disconnected;
                        return Err(CaptureError::Connection);
                    }
                    Err(error) => return Err(error),
                }
            }
            let connection =
                match std::mem::replace(&mut self.state, CaptureState::CleanupUnconfirmed) {
                    CaptureState::Connected(connection) => connection,
                    CaptureState::CleanupUnconfirmed => {
                        return Err(CaptureError::CleanupUnconfirmed);
                    }
                    CaptureState::Closed => {
                        self.state = CaptureState::Closed;
                        return Err(CaptureError::Closed);
                    }
                    CaptureState::Disconnected => unreachable!(),
                };
            let mut connection = connection;
            let captured = match metrics {
                Some((metrics, operation)) => {
                    connection.capture_with_metrics(operation, metrics).await
                }
                None => connection.capture().await,
            };
            match captured {
                Ok(mut image) => {
                    self.state = CaptureState::Connected(connection);
                    image.recovered = recovered;
                    return Ok(image);
                }
                Err(error) if error.cleanup_unconfirmed() => {
                    // Closing SSH does not prove the remote capture command cleaned up.
                    let _ = measure_capture_stage(
                        metrics,
                        Stage::ConnectionCleanup,
                        connection.close(),
                    )
                    .await;
                    return Err(error);
                }
                Err(error) if error.retryable() => {
                    measure_capture_stage(metrics, Stage::ConnectionCleanup, connection.close())
                        .await?;
                    self.state = CaptureState::Disconnected;
                    if attempt == 1 {
                        return Err(error);
                    }
                }
                Err(error) => {
                    self.state = CaptureState::Connected(connection);
                    return Err(error);
                }
            }
        }
        unreachable!()
    }

    async fn close(&mut self) -> Result<(), CaptureError> {
        match std::mem::replace(&mut self.state, CaptureState::CleanupUnconfirmed) {
            CaptureState::Connected(connection) => {
                connection.close().await?;
                self.state = CaptureState::Closed;
                Ok(())
            }
            CaptureState::Disconnected | CaptureState::Closed => {
                self.state = CaptureState::Closed;
                Ok(())
            }
            CaptureState::CleanupUnconfirmed => Err(CaptureError::CleanupUnconfirmed),
        }
    }
}

pub struct RecoveringCapture(Recovery<CaptureSession>);

impl RecoveringCapture {
    pub async fn connect(config: CaptureConfig) -> Result<Self, CaptureError> {
        Recovery::connect(config).await.map(Self)
    }
    pub async fn capture(&mut self) -> Result<CapturedImage, CaptureError> {
        self.0.capture().await
    }
    pub async fn capture_with_metrics(
        &mut self,
        operation: Operation,
        metrics: &Metrics,
    ) -> Result<CapturedImage, CaptureError> {
        self.0.capture_inner(Some((metrics, operation))).await
    }
    pub async fn close(&mut self) -> Result<(), CaptureError> {
        self.0.close().await
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod metrics_tests {
    use super::*;
    use std::task::{Context, Poll, Waker};

    #[tokio::test]
    async fn stages_record_success_error_and_timeout_separately() {
        let metrics = Metrics::default();
        let context = Some((&metrics, Operation::Observe));
        assert_eq!(
            capture_stage(
                context,
                Stage::CaptureRequest,
                tokio::time::Instant::now() + Duration::from_secs(1),
                async { Ok(7) },
            )
            .await,
            Ok(7)
        );
        assert_eq!(
            capture_stage(
                context,
                Stage::ImageTransfer,
                tokio::time::Instant::now() + Duration::from_secs(1),
                async { Err::<(), _>(CaptureError::TransferLimit) },
            )
            .await,
            Err(CaptureError::TransferLimit)
        );
        assert_eq!(
            capture_stage(
                context,
                Stage::CaptureRequest,
                tokio::time::Instant::now(),
                std::future::pending::<Result<(), CaptureError>>(),
            )
            .await,
            Err(CaptureError::Timeout)
        );
        let snapshot = metrics.snapshot();
        let stages = &snapshot.stages[&Operation::Observe];
        let request = &stages[&Stage::CaptureRequest];
        assert_eq!(request.count, 2);
        assert_eq!(request.outcomes.success, 1);
        assert_eq!(request.outcomes.timeout, 1);
        assert_eq!(request.outcomes.failure, 0);
        assert_eq!(request.outcomes.cancelled, 0);
        assert_eq!(stages[&Stage::ImageTransfer].count, 1);
        assert_eq!(stages[&Stage::ImageTransfer].outcomes.failure, 1);
        assert_eq!(stages[&Stage::ImageValidation].count, 0);
        assert_eq!(snapshot.operations[&Operation::Observe].count, 0);
    }

    #[tokio::test]
    async fn cancelled_capture_stage_counts_once_without_finishing() {
        let metrics = Metrics::default();
        let mut stage = Box::pin(capture_stage(
            Some((&metrics, Operation::Press)),
            Stage::ImageTransfer,
            tokio::time::Instant::now() + Duration::from_secs(1),
            std::future::pending::<Result<(), CaptureError>>(),
        ));
        assert_eq!(
            stage.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        );
        assert_eq!(
            metrics.snapshot().stages[&Operation::Press][&Stage::ImageTransfer].count,
            0
        );
        drop(stage);
        let snapshot = metrics.snapshot();
        let transfer = &snapshot.stages[&Operation::Press][&Stage::ImageTransfer];
        assert_eq!(transfer.count, 1);
        assert_eq!(transfer.outcomes.cancelled, 1);
        assert_eq!(transfer.outcomes.timeout, 0);
        assert_eq!(transfer.outcomes.success, 0);
        assert_eq!(transfer.outcomes.failure, 0);
    }

    #[tokio::test]
    async fn validation_timings_preserve_decoder_errors_and_respect_disabled_metrics() {
        for enabled in [true, false] {
            let metrics = Metrics::new(enabled);
            let result = capture_stage(
                Some((&metrics, Operation::Launch)),
                Stage::ImageValidation,
                tokio::time::Instant::now() + Duration::from_secs(1),
                async { decode_image(b"not a PNG".to_vec()) },
            )
            .await;
            assert!(matches!(result, Err(CaptureError::InvalidImage)));
            let snapshot = metrics.snapshot();
            let validation = &snapshot.stages[&Operation::Launch][&Stage::ImageValidation];
            assert_eq!(validation.count, u64::from(enabled));
            assert_eq!(validation.outcomes.failure, u64::from(enabled));
            assert_eq!(validation.outcomes.timeout, 0);
            assert_eq!(validation.outcomes.cancelled, 0);
        }
    }
}
