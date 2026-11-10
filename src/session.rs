use crate::setup;
use fs2::FileExt;
use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use webos_mcp::{
    capture::{CaptureConfig, CapturedImage, RecoveringCapture},
    error::{TvError, outcome},
    metrics::{Metrics, Operation, Outcome, Stage},
    transport::{self, AuthenticatedSocket},
};

pub(crate) fn diagnostic(stage: &'static str) {
    if std::env::var_os("WEBOS_MCP_DIAGNOSTICS").as_deref() == Some(std::ffi::OsStr::new("1")) {
        eprintln!("webos-mcp: {stage}");
    }
}

pub trait Device: Send {
    fn probe(&mut self) -> impl std::future::Future<Output = Result<(), TvError>> + Send;
    fn send(
        &mut self,
        action: &Action,
    ) -> impl std::future::Future<Output = Result<(), TvError>> + Send;
    fn capture(
        &mut self,
    ) -> impl std::future::Future<Output = Result<CapturedImage, TvError>> + Send;
    fn capture_with_metrics(
        &mut self,
        op: Operation,
        metrics: &Metrics,
    ) -> impl std::future::Future<Output = Result<CapturedImage, TvError>> + Send {
        async move {
            let measurement = metrics.stage(op, Stage::CaptureRequest);
            let result = self.capture().await;
            measurement.finish(outcome(&result));
            result
        }
    }
    fn close(&mut self) -> impl std::future::Future<Output = Result<(), TvError>> + Send;
}

pub struct LiveDevice {
    remote: Option<AuthenticatedSocket>,
    input: Option<AuthenticatedSocket>,
    capture: Option<RecoveringCapture>,
    cleanup_unconfirmed: bool,
}
impl LiveDevice {
    async fn connect_control(
        &mut self,
        endpoint: SocketAddr,
        fingerprint: [u8; 32],
        metrics: &Metrics,
    ) -> Result<(), TvError> {
        self.cleanup_unconfirmed = true;
        let remote = transport::connect(endpoint, fingerprint, metrics).await;
        // A completed failed handshake drops its local socket. Cancellation does
        // not reach this point and must retain uncertainty.
        self.cleanup_unconfirmed = false;
        self.remote = Some(remote?);
        Ok(())
    }
}
impl Device for LiveDevice {
    async fn probe(&mut self) -> Result<(), TvError> {
        let input = self.input.as_mut().ok_or("TV input is unavailable")?;
        let remote = self.remote.as_mut().ok_or("TV control is unavailable")?;
        // The sockets are independent; check both in one round trip.
        let (input, remote) = tokio::join!(input.probe(), remote.probe());
        input?;
        remote
    }
    async fn send(&mut self, action: &Action) -> Result<(), TvError> {
        match action {
            Action::Press { button, count } => {
                for i in 0..*count {
                    if i > 0 {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                    }
                    self.input
                        .as_mut()
                        .ok_or("TV input is unavailable")?
                        .send_button(button)
                        .await?;
                }
                Ok(())
            }
            Action::Launch { app_id } => {
                self.remote
                    .as_mut()
                    .ok_or("TV control is unavailable")?
                    .launch(app_id)
                    .await
            }
        }
    }
    async fn capture(&mut self) -> Result<CapturedImage, TvError> {
        self.capture
            .as_mut()
            .ok_or("Visual capture is unavailable")?
            .capture()
            .await
            .map_err(TvError::from)
    }
    async fn capture_with_metrics(
        &mut self,
        op: Operation,
        metrics: &Metrics,
    ) -> Result<CapturedImage, TvError> {
        self.capture
            .as_mut()
            .ok_or("Visual capture is unavailable")?
            .capture_with_metrics(op, metrics)
            .await
            .map_err(TvError::from)
    }
    async fn close(&mut self) -> Result<(), TvError> {
        let prior_uncertainty = self.cleanup_unconfirmed;
        self.cleanup_unconfirmed = true;
        let input = match self.input.take() {
            Some(s) => s.close().await,
            None => Ok(()),
        };
        let remote = match self.remote.take() {
            Some(s) => s.close().await,
            None => Ok(()),
        };
        let capture = match self.capture.as_mut() {
            Some(s) => s.close().await.map_err(TvError::from),
            None => Ok(()),
        };
        if prior_uncertainty || input.is_err() || remote.is_err() || capture.is_err() {
            // The sockets are gone and the flag never clears, so only a new
            // process can release this session. Say so instead of implying a
            // later disconnect could succeed.
            let message =
                "TV cleanup could not be confirmed; restart the MCP server before reconnecting";
            let timed_out = [input, remote, capture]
                .iter()
                .any(|closed| outcome(closed) == Outcome::Timeout);
            return Err(if timed_out {
                TvError::timeout(message)
            } else {
                TvError::failure(message)
            });
        }
        self.cleanup_unconfirmed = false;
        Ok(())
    }
}

pub struct SessionOwner<D = LiveDevice> {
    requests: Arc<Mutex<()>>,
    state: Arc<Mutex<State<D>>>,
    metrics: Metrics,
}
impl<D> Clone for SessionOwner<D> {
    fn clone(&self) -> Self {
        Self {
            requests: self.requests.clone(),
            state: self.state.clone(),
            metrics: self.metrics.clone(),
        }
    }
}
impl Default for SessionOwner<LiveDevice> {
    fn default() -> Self {
        Self::new(Metrics::new(true))
    }
}
enum State<D> {
    Disconnected,
    Connected(Box<Connected<D>>),
    Closing(Box<Connected<D>>),
}
struct Connected<D> {
    _lock: File,
    device: D,
    view: View,
    baseline: Option<[u8; 32]>,
    healthy: bool,
    marker: PathBuf,
}
#[derive(Debug, PartialEq, Eq)]
enum View {
    NeedsObservation,
    Fresh(String),
    Uncertain,
}
#[derive(Clone, Copy)]
pub struct ObserveOptions {
    pub wait: Duration,
    pub change_timeout: Duration,
}
impl Default for ObserveOptions {
    fn default() -> Self {
        Self {
            wait: Duration::ZERO,
            change_timeout: Duration::ZERO,
        }
    }
}
pub enum Action {
    Press { button: String, count: u8 },
    Launch { app_id: String },
}
impl Action {
    fn operation(&self) -> Operation {
        match self {
            Self::Press { .. } => Operation::Press,
            Self::Launch { .. } => Operation::Launch,
        }
    }
}
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VisualChange {
    First,
    Changed,
    Unchanged,
    Black,
}
pub struct Observation {
    pub image: CapturedImage,
    pub id: String,
    pub visual_change: VisualChange,
    pub delivery_unresolved: bool,
    pub samples: u32,
    pub elapsed_ms: u64,
}
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Sent,
    Unknown,
}
pub struct ActionReport {
    pub delivery: Delivery,
    pub error: Option<TvError>,
    pub observation: Option<Observation>,
    pub observation_error: Option<TvError>,
}
const UNRESOLVED: &str = "An earlier action has unresolved delivery; actions are blocked until the user checks the TV and runs `webos-mcp acknowledge-delivery`";

fn observation_id() -> Result<String, &'static str> {
    let mut bytes = [0u8; 16];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut bytes)
        .map_err(|_| "Could not create observation ID")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn state_dir() -> Result<PathBuf, &'static str> {
    let path = directories::ProjectDirs::from("", "", "webos-mcp")
        .ok_or("Local action state is unavailable")?
        .data_local_dir()
        .to_owned();
    std::fs::create_dir_all(&path).map_err(|_| "Local action state is unavailable")?;
    Ok(path)
}

fn read_marker(path: &Path) -> Result<Option<bool>, &'static str> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes != b"C")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("Local action state is unavailable"),
    }
}

/// Make the marker's directory entry durable. Needed once after the file is
/// created, not on every one-byte update of an existing file.
fn sync_marker_directory(path: &Path) -> Result<(), &'static str> {
    #[cfg(unix)]
    File::open(path.parent().ok_or("Local action state is unavailable")?)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "Could not save local action state")?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Durably update the marker's content. The caller must already have made the
/// file's directory entry durable through `load_or_migrate_marker`.
fn write_marker(path: &Path, pending: bool) -> Result<(), &'static str> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|_| "Could not save local action state")?;
    file.write_all(if pending { b"P" } else { b"C" })
        .map_err(|_| "Could not save local action state")?;
    file.set_len(1)
        .map_err(|_| "Could not save local action state")?;
    file.sync_all()
        .map_err(|_| "Could not save local action state")?;
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "Could not verify local action state")?;
    let mut byte = [0];
    file.read_exact(&mut byte)
        .map_err(|_| "Could not verify local action state")?;
    if byte[0] != if pending { b'P' } else { b'C' } {
        return Err("Could not verify local action state");
    }
    Ok(())
}

fn unresolved() -> Result<bool, &'static str> {
    let path = state_dir()?.join("action-state");
    load_or_migrate_marker(&path, || {
        let entry = keyring::Entry::new("webos-mcp", "action-pending")
            .map_err(|_| "Secure action state is unavailable")?;
        match entry.get_password() {
            Ok(value) if value == "clear" => Ok(false),
            Ok(_) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(_) => Err("Could not migrate secure action state"),
        }
    })
}

fn load_or_migrate_marker(
    path: &Path,
    legacy: impl FnOnce() -> Result<bool, &'static str>,
) -> Result<bool, &'static str> {
    let pending = match read_marker(path)? {
        Some(pending) => pending,
        None => {
            let pending = legacy()?;
            write_marker(path, pending)?;
            pending
        }
    };
    // Once per load, so later one-byte updates need only sync the file.
    sync_marker_directory(path)?;
    Ok(pending)
}

/// Clear the durable delivery guard after the user has checked the TV.
///
/// This is the only path that clears an unresolved delivery, and it is never
/// reachable from an MCP tool: the server cannot observe whether a late button
/// could still arrive, so a person has to accept that risk explicitly. The
/// caller must hold the session lock so no connected session is using the
/// marker.
pub fn acknowledge_delivery(
    _lock: &File,
    confirmed: impl FnOnce() -> Result<bool, &'static str>,
) -> Result<Acknowledgement, &'static str> {
    acknowledge_marker(&state_dir()?.join("action-state"), unresolved()?, confirmed)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Acknowledgement {
    NothingUnresolved,
    Declined,
    Cleared,
}

fn acknowledge_marker(
    path: &Path,
    pending: bool,
    confirmed: impl FnOnce() -> Result<bool, &'static str>,
) -> Result<Acknowledgement, &'static str> {
    if !pending {
        return Ok(Acknowledgement::NothingUnresolved);
    }
    if !confirmed()? {
        return Ok(Acknowledgement::Declined);
    }
    write_marker(path, false)?;
    sync_marker_directory(path)?;
    Ok(Acknowledgement::Cleared)
}
pub fn acquire_lock() -> Result<File, &'static str> {
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(state_dir()?.join("session.lock"))
        .map_err(|_| "Could not acquire the TV session lock")?;
    lock.try_lock_exclusive()
        .map_err(|_| "Another server owns the TV session")?;
    Ok(lock)
}

impl SessionOwner<LiveDevice> {
    pub fn new(metrics: Metrics) -> Self {
        Self {
            requests: Arc::new(Mutex::new(())),
            state: Arc::new(Mutex::new(State::Disconnected)),
            metrics,
        }
    }
    pub async fn connect(&self) -> Result<&'static str, TvError> {
        let _request = self.requests.try_lock().map_err(|_| "TV session is busy")?;
        let mut state = self.state.lock().await;
        match &mut *state {
            State::Closing(_) => {
                return Err(TvError::failure(
                    "TV cleanup is incomplete; call tv_disconnect, and restart the MCP server if it cannot confirm cleanup",
                ));
            }
            State::Connected(s) => {
                if !s.healthy || s.device.probe().await.is_err() {
                    s.healthy = false;
                    if s.view != View::Uncertain {
                        s.view = View::NeedsObservation;
                    }
                    return Err(TvError::failure(
                        "TV connection is stale; disconnect, reconnect and observe again",
                    ));
                }
                return Ok("TV is already connected");
            }
            State::Disconnected => {}
        }
        let lock = acquire_lock()?;
        let pending = unresolved()?;
        let measurement = self
            .metrics
            .stage(Operation::Connect, Stage::CredentialLookup);
        let loaded = setup::load_device_credentials();
        measurement.finish(outcome(&loaded));
        let (address, mut credentials) = loaded?;
        let endpoint = SocketAddr::new(address, 3001);
        // Setup owns its resources in `Closing` until `finish_setup` resolves it.
        *state = State::Closing(Box::new(Connected {
            _lock: lock,
            device: LiveDevice {
                remote: None,
                input: None,
                capture: None,
                cleanup_unconfirmed: false,
            },
            view: if pending {
                View::Uncertain
            } else {
                View::NeedsObservation
            },
            baseline: None,
            healthy: true,
            marker: state_dir()?.join("action-state"),
        }));
        let established = self
            .establish(&mut state, endpoint, address, &mut credentials)
            .await;
        let has_capture = state.finish_setup(established).await?;
        Ok(if pending {
            "Connected; an earlier action has unresolved delivery. Actions are blocked until the user checks the TV and runs `webos-mcp acknowledge-delivery` outside this session."
        } else if has_capture {
            "Connected; capture is configured. Observe before acting."
        } else {
            "Connected; visual capture is unavailable. Actions are blocked."
        })
    }
    async fn establish(
        &self,
        state: &mut State<LiveDevice>,
        endpoint: SocketAddr,
        address: std::net::IpAddr,
        credentials: &mut setup::Credentials,
    ) -> Result<bool, TvError> {
        let State::Closing(s) = state else {
            unreachable!()
        };
        let measurement = self
            .metrics
            .stage(Operation::Connect, Stage::ConnectionSetup);
        let remote = s
            .device
            .connect_control(endpoint, credentials.fingerprint, &self.metrics)
            .await;
        measurement.finish(outcome(&remote));
        remote?;
        let measurement = self.metrics.stage(Operation::Connect, Stage::Registration);
        let key = transport::register(
            s.device.remote.as_mut().unwrap(),
            Some(&credentials.client_key),
        )
        .await;
        measurement.finish(outcome(&key));
        setup::persist_verified_credentials(credentials, address, key?)?;
        s.device.cleanup_unconfirmed = true;
        let measurement = self
            .metrics
            .stage(Operation::Connect, Stage::ConnectionSetup);
        let input = s
            .device
            .remote
            .as_mut()
            .unwrap()
            .input_socket(endpoint, &self.metrics)
            .await;
        measurement.finish(outcome(&input));
        s.device.cleanup_unconfirmed = false;
        s.device.input = Some(input?);
        if let Some(config) = credentials.ssh.take() {
            s.device.cleanup_unconfirmed = true;
            let capture = RecoveringCapture::connect(CaptureConfig {
                endpoint: SocketAddr::new(address, config.port),
                username: config.username,
                agent_socket: config.agent_socket,
                public_key: config.public_key,
            })
            .await;
            if let Err(error) = &capture
                && !error.cleanup_unconfirmed()
            {
                s.device.cleanup_unconfirmed = false;
            }
            s.device.capture = Some(capture?);
            s.device.cleanup_unconfirmed = false;
        }
        Ok(s.device.capture.is_some())
    }
}

impl<D: Device> State<D> {
    /// Resolve a setup attempt that ran to completion. A known failure releases
    /// what it owns and returns to `Disconnected` only when cleanup is confirmed.
    /// A cancelled setup never reaches this and stays `Closing`.
    async fn finish_setup<T>(&mut self, result: Result<T, TvError>) -> Result<T, TvError> {
        let State::Closing(s) = self else {
            unreachable!()
        };
        match result {
            Ok(value) => {
                let State::Closing(s) = std::mem::replace(self, State::Disconnected) else {
                    unreachable!()
                };
                *self = State::Connected(s);
                Ok(value)
            }
            Err(error) => {
                if s.device.close().await.is_ok() {
                    *self = State::Disconnected;
                }
                Err(error)
            }
        }
    }
}

impl<D: Device + 'static> SessionOwner<D> {
    pub fn start_keepalive(&self) -> tokio::task::JoinHandle<()> {
        let owner = Arc::downgrade(&self.state);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                let Some(owner) = owner.upgrade() else { break };
                let Ok(mut state) = owner.try_lock() else {
                    continue;
                };
                let State::Connected(s) = &mut *state else {
                    continue;
                };
                if s.healthy && s.device.probe().await.is_err() {
                    s.healthy = false;
                    if s.view != View::Uncertain {
                        s.view = View::NeedsObservation;
                    }
                }
            }
        })
    }
    async fn observe_locked(
        s: &mut Connected<D>,
        options: ObserveOptions,
        metrics: &Metrics,
        op: Operation,
    ) -> Result<Observation, TvError> {
        let started = Instant::now();
        let uncertain = s.view == View::Uncertain;
        if !uncertain {
            s.view = View::NeedsObservation;
        }
        tokio::time::sleep(options.wait).await;
        let sample_start = Instant::now();
        let mut samples = 0;
        let mut recovered = false;
        let (image, visual_change) = loop {
            let captured = s.device.capture_with_metrics(op, metrics).await;
            let mut image = captured?;
            recovered |= image.recovered;
            image.recovered = recovered;
            samples += 1;
            let change = if image.all_black {
                VisualChange::Black
            } else {
                match s.baseline {
                    None => VisualChange::First,
                    Some(hash) if hash == image.fingerprint => VisualChange::Unchanged,
                    Some(_) => VisualChange::Changed,
                }
            };
            if change != VisualChange::Unchanged
                || samples >= 4
                || sample_start.elapsed() >= options.change_timeout
            {
                break (image, change);
            }
            tokio::time::sleep(
                Duration::from_millis(150).min(
                    options
                        .change_timeout
                        .saturating_sub(sample_start.elapsed()),
                ),
            )
            .await;
            if sample_start.elapsed() >= options.change_timeout {
                break (image, change);
            }
        };
        let id = observation_id()?;
        s.baseline = Some(image.fingerprint);
        if !uncertain {
            s.view = View::Fresh(id.clone());
        }
        Ok(Observation {
            image,
            id,
            visual_change,
            delivery_unresolved: uncertain,
            samples,
            elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        })
    }
    pub async fn observe(&self, options: ObserveOptions) -> Result<Observation, TvError> {
        let _request = self.requests.try_lock().map_err(|_| "TV session is busy")?;
        let mut state = self.state.lock().await;
        match &mut *state {
            State::Connected(s) => {
                Self::observe_locked(s, options, &self.metrics, Operation::Observe).await
            }
            State::Closing(_) => Err(TvError::failure("TV cleanup is incomplete")),
            State::Disconnected => Err(TvError::failure("TV is disconnected")),
        }
    }
    pub async fn act(
        &self,
        action: Action,
        id: &str,
        observe: Option<ObserveOptions>,
    ) -> Result<ActionReport, TvError> {
        let _request = self.requests.try_lock().map_err(|_| "TV session is busy")?;
        let mut state = self.state.lock().await;
        let s = match &mut *state {
            State::Connected(s) => s,
            State::Closing(_) => return Err(TvError::failure("TV cleanup is incomplete")),
            State::Disconnected => return Err(TvError::failure("TV is disconnected")),
        };
        match &s.view {
            View::Fresh(current) if current == id => {}
            View::Fresh(_) => {
                return Err(TvError::failure(
                    "Observation ID is stale; observe before acting",
                ));
            }
            View::NeedsObservation => {
                return Err(TvError::failure("Observe the TV before an action"));
            }
            View::Uncertain => return Err(TvError::failure(UNRESOLVED)),
        }
        if !s.healthy {
            return Err(TvError::failure(
                "TV connection is stale; disconnect, reconnect and observe again",
            ));
        }
        s.view = View::NeedsObservation;
        let op = action.operation();
        let measurement = self.metrics.stage(op, Stage::InputConnection);
        let probe = s.device.probe().await;
        measurement.finish(outcome(&probe));
        if let Err(error) = probe {
            s.healthy = false;
            return Err(error);
        }
        // Persist before any send; cancellation after this point blocks further actions.
        if let Err(error) = write_marker(&s.marker, true) {
            // Nothing was sent, so this session stays usable after a new
            // observation. The file may still hold a pending byte; restore it
            // when possible, and otherwise a restart fails closed.
            let _ = write_marker(&s.marker, false);
            return Err(error.into());
        }
        s.view = View::Uncertain;
        let measurement = self.metrics.stage(op, Stage::CommandDelivery);
        let sent = s.device.send(&action).await;
        measurement.finish(outcome(&sent));
        if let Err(error) = sent {
            return Ok(ActionReport {
                delivery: Delivery::Unknown,
                error: Some(error),
                observation: None,
                observation_error: None,
            });
        }
        if let Err(error) = write_marker(&s.marker, false) {
            return Ok(ActionReport {
                delivery: Delivery::Unknown,
                error: Some(error.into()),
                observation: None,
                observation_error: None,
            });
        }
        s.view = View::NeedsObservation;
        let (observation, observation_error) = match observe {
            Some(options) => match Self::observe_locked(s, options, &self.metrics, op).await {
                Ok(o) => (Some(o), None),
                Err(e) => (None, Some(e)),
            },
            None => (None, None),
        };
        Ok(ActionReport {
            delivery: Delivery::Sent,
            error: None,
            observation,
            observation_error,
        })
    }
    pub async fn disconnect(&self) -> Result<&'static str, TvError> {
        let _request = self.requests.try_lock().map_err(|_| "TV session is busy")?;
        let mut state = self.state.lock().await;
        if matches!(&*state, State::Disconnected) {
            return Ok("TV is already disconnected");
        }
        if matches!(&*state, State::Connected(_)) {
            let State::Connected(s) = std::mem::replace(&mut *state, State::Disconnected) else {
                unreachable!()
            };
            *state = State::Closing(s);
        }
        let State::Closing(s) = &mut *state else {
            unreachable!()
        };
        let measurement = self
            .metrics
            .stage(Operation::Disconnect, Stage::ConnectionCleanup);
        let closed = s.device.close().await;
        measurement.finish(outcome(&closed));
        closed?;
        *state = State::Disconnected;
        Ok("TV disconnected")
    }
}

#[cfg(test)]
mod connection_tests;
#[cfg(test)]
mod tests;
