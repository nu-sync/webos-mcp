use super::*;
#[test]
fn durable_marker_fails_closed_for_interrupted_or_invalid_state() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("action-state");
    assert_eq!(read_marker(&path).unwrap(), None);
    write_marker(&path, false).unwrap();
    assert_eq!(read_marker(&path).unwrap(), Some(false));
    write_marker(&path, true).unwrap();
    assert_eq!(read_marker(&path).unwrap(), Some(true));
    std::fs::write(&path, b"").unwrap();
    assert_eq!(read_marker(&path).unwrap(), Some(true));
    std::fs::write(&path, b"invalid").unwrap();
    assert_eq!(read_marker(&path).unwrap(), Some(true));
    write_marker(&path, false).unwrap();
    assert_eq!(read_marker(&path).unwrap(), Some(false));
}

#[test]
fn marker_migration_reads_legacy_only_once_and_preserves_pending() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("action-state");
    assert_eq!(load_or_migrate_marker(&path, || Ok(true)), Ok(true));
    assert_eq!(
        load_or_migrate_marker(&path, || Err("unexpected legacy read")),
        Ok(true)
    );
    write_marker(&path, false).unwrap();
    assert_eq!(
        load_or_migrate_marker(&path, || Err("unexpected legacy read")),
        Ok(false)
    );
}

#[test]
fn failed_migration_does_not_create_a_clear_marker() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("action-state");
    assert!(load_or_migrate_marker(&path, || Err("store unavailable")).is_err());
    assert_eq!(read_marker(&path).unwrap(), None);
}

use std::{collections::VecDeque, sync::Mutex as StdMutex};

enum CaptureStep {
    Frame(u8),
    Recovered(u8),
    Black,
    Error,
    Pending,
}
enum IoStep {
    Ok,
    Error,
    Pending,
}
#[derive(Default)]
struct Script {
    captures: VecDeque<CaptureStep>,
    sends: VecDeque<IoStep>,
    probes: VecDeque<IoStep>,
    closes: VecDeque<IoStep>,
    calls: Vec<&'static str>,
    buttons: Vec<(String, u8)>,
    pending_at_send: Vec<bool>,
    cleanup_unconfirmed: bool,
}
struct FakeDevice {
    script: Arc<StdMutex<Script>>,
    marker: PathBuf,
    capture_started: Arc<tokio::sync::Notify>,
}
impl Device for FakeDevice {
    async fn establish(&mut self, _: &Metrics) -> Result<bool, TvError> {
        Ok(true)
    }
    async fn probe(&mut self) -> Result<(), TvError> {
        let step = {
            let mut script = self.script.lock().unwrap();
            script.calls.push("probe");
            script.probes.pop_front().unwrap_or(IoStep::Ok)
        };
        match step {
            IoStep::Ok => Ok(()),
            IoStep::Error => Err("probe failed".into()),
            IoStep::Pending => std::future::pending().await,
        }
    }
    async fn send(&mut self, action: &Action) -> Result<(), TvError> {
        let step = {
            let mut script = self.script.lock().unwrap();
            script.calls.push("send");
            if let Action::Press { button, count } = action {
                script.buttons.push((button.clone(), *count));
            }
            script
                .pending_at_send
                .push(read_marker(&self.marker).unwrap() == Some(true));
            script.sends.pop_front().unwrap_or(IoStep::Ok)
        };
        match step {
            IoStep::Ok => Ok(()),
            IoStep::Error => Err("send outcome unknown".into()),
            IoStep::Pending => std::future::pending().await,
        }
    }
    async fn capture(&mut self) -> Result<CapturedImage, TvError> {
        let step = {
            let mut script = self.script.lock().unwrap();
            script.calls.push("capture");
            script.captures.pop_front().expect("unexpected capture")
        };
        self.capture_started.notify_one();
        match step {
            CaptureStep::Frame(value) => Ok(frame(value, false)),
            CaptureStep::Recovered(value) => {
                let mut image = frame(value, false);
                image.recovered = true;
                Ok(image)
            }
            CaptureStep::Black => Ok(frame(0, true)),
            CaptureStep::Error => Err("capture failed".into()),
            CaptureStep::Pending => std::future::pending().await,
        }
    }
    async fn close(&mut self) -> Result<(), TvError> {
        let step = {
            let mut script = self.script.lock().unwrap();
            script.calls.push("close");
            if script.cleanup_unconfirmed {
                return Err("cleanup unconfirmed".into());
            }
            script.cleanup_unconfirmed = true;
            script.closes.pop_front().unwrap_or(IoStep::Ok)
        };
        match step {
            IoStep::Ok => {
                self.script.lock().unwrap().cleanup_unconfirmed = false;
                Ok(())
            }
            IoStep::Error => Err("cleanup unconfirmed".into()),
            IoStep::Pending => std::future::pending().await,
        }
    }
}
fn frame(value: u8, all_black: bool) -> CapturedImage {
    CapturedImage {
        bytes: vec![value],
        mime_type: "image/png",
        all_black,
        fingerprint: [value; 32],
        width: 1,
        height: 1,
        recovered: false,
    }
}
struct Fixture {
    owner: SessionOwner<FakeDevice>,
    script: Arc<StdMutex<Script>>,
    marker: PathBuf,
    capture_started: Arc<tokio::sync::Notify>,
    _directory: tempfile::TempDir,
}
fn fixture(captures: Vec<CaptureStep>) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("action-state");
    write_marker(&marker, false).unwrap();
    let script = Arc::new(StdMutex::new(Script {
        captures: captures.into(),
        ..Default::default()
    }));
    let capture_started = Arc::new(tokio::sync::Notify::new());
    let device = FakeDevice {
        script: script.clone(),
        marker: marker.clone(),
        capture_started: capture_started.clone(),
    };
    let owner = SessionOwner {
        requests: Arc::new(Mutex::new(())),
        state: Arc::new(Mutex::new(State::Connected(Box::new(Connected {
            _lock: File::create(directory.path().join("session.lock")).unwrap(),
            device,
            view: View::NeedsObservation,
            baseline: None,
            healthy: true,
            marker: marker.clone(),
        })))),
        metrics: Metrics::new(true),
        state_dir: StateDir::at(directory.path().to_owned()),
        make: Arc::new(|| unreachable!("fixtures start connected")),
    };
    Fixture {
        owner,
        script,
        marker,
        capture_started,
        _directory: directory,
    }
}
fn right() -> Action {
    Action::Press {
        button: "RIGHT".into(),
        count: 1,
    }
}

#[tokio::test]
async fn stale_observation_is_rejected_before_probe_or_send() {
    let f = fixture(vec![
        CaptureStep::Frame(1),
        CaptureStep::Frame(2),
        CaptureStep::Frame(3),
    ]);
    let first = f.owner.observe(ObserveOptions::default()).await.unwrap();
    let second = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(second.visual_change, VisualChange::Changed);
    assert_eq!(
        f.owner.act(right(), &first.id, None).await.err().unwrap(),
        "Observation ID is stale; observe before acting"
    );
    assert_eq!(f.script.lock().unwrap().calls, ["capture", "capture"]);
    let report = f
        .owner
        .act(right(), &second.id, Some(ObserveOptions::default()))
        .await
        .unwrap();
    assert_eq!(report.delivery, Delivery::Sent);
    assert_eq!(report.observation.unwrap().image.bytes, [3]);
    assert_eq!(f.script.lock().unwrap().buttons, [("RIGHT".into(), 1)]);
}

#[tokio::test]
async fn combined_action_reports_send_success_separately_from_capture_failure() {
    let f = fixture(vec![
        CaptureStep::Frame(1),
        CaptureStep::Error,
        CaptureStep::Frame(2),
    ]);
    let before = f.owner.observe(ObserveOptions::default()).await.unwrap();
    let report = f
        .owner
        .act(right(), &before.id, Some(ObserveOptions::default()))
        .await
        .unwrap();
    assert_eq!(report.delivery, Delivery::Sent);
    assert_eq!(report.error, None);
    assert!(report.observation.is_none());
    assert_eq!(report.observation_error, Some("capture failed".into()));
    assert_eq!(read_marker(&f.marker), Ok(Some(false)));
    assert_eq!(
        f.owner.act(right(), &before.id, None).await.err().unwrap(),
        "Observe the TV before an action"
    );
    let after = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert_eq!(after.image.bytes, [2]);
    let script = f.script.lock().unwrap();
    assert_eq!(script.buttons, [("RIGHT".into(), 1)]);
    assert_eq!(
        script.calls,
        ["capture", "probe", "send", "capture", "capture"]
    );
    assert_eq!(script.pending_at_send, [true]);
}

#[tokio::test]
async fn unknown_delivery_survives_an_observation_and_blocks_resend() {
    let f = fixture(vec![CaptureStep::Frame(1), CaptureStep::Frame(2)]);
    f.script.lock().unwrap().sends.push_back(IoStep::Error);
    let before = f.owner.observe(ObserveOptions::default()).await.unwrap();
    let report = f
        .owner
        .act(right(), &before.id, Some(ObserveOptions::default()))
        .await
        .unwrap();
    assert_eq!(report.delivery, Delivery::Unknown);
    assert_eq!(report.error, Some("send outcome unknown".into()));
    assert!(report.observation.is_none());
    assert_eq!(read_marker(&f.marker), Ok(Some(true)));
    let after = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert!(after.delivery_unresolved);
    assert_eq!(
        f.owner.act(right(), &after.id, None).await.err().unwrap(),
        UNRESOLVED
    );
    assert_eq!(f.script.lock().unwrap().buttons, [("RIGHT".into(), 1)]);
    assert_eq!(f.owner.disconnect().await, Ok("TV disconnected"));
    assert_eq!(read_marker(&f.marker), Ok(Some(true)));
}

#[tokio::test]
async fn cancelled_send_leaves_a_durable_guard() {
    let f = fixture(vec![CaptureStep::Frame(1), CaptureStep::Frame(2)]);
    f.script.lock().unwrap().sends.push_back(IoStep::Pending);
    let before = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(5),
            f.owner.act(right(), &before.id, None)
        )
        .await
        .is_err()
    );
    assert_eq!(read_marker(&f.marker), Ok(Some(true)));
    let after = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert!(after.delivery_unresolved);
    assert!(f.owner.act(right(), &after.id, None).await.is_err());
    let script = f.script.lock().unwrap();
    assert_eq!(script.buttons, [("RIGHT".into(), 1)]);
    assert_eq!(script.pending_at_send, [true]);
}

#[tokio::test]
async fn cancelled_observation_invalidates_the_previous_id() {
    let f = fixture(vec![CaptureStep::Frame(1), CaptureStep::Frame(2)]);
    let before = f.owner.observe(ObserveOptions::default()).await.unwrap();
    let options = ObserveOptions {
        wait: Duration::from_secs(1),
        ..ObserveOptions::default()
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(1), f.owner.observe(options))
            .await
            .is_err()
    );
    assert_eq!(
        f.owner.act(right(), &before.id, None).await.err().unwrap(),
        "Observe the TV before an action"
    );
    let after = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert_ne!(after.id, before.id);
    assert_eq!(f.script.lock().unwrap().calls, ["capture", "capture"]);
}

#[tokio::test]
async fn failed_preflight_invalidates_view_without_creating_pending_delivery() {
    let f = fixture(vec![CaptureStep::Frame(1)]);
    f.script.lock().unwrap().probes.push_back(IoStep::Error);
    let before = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert_eq!(
        f.owner.act(right(), &before.id, None).await.err().unwrap(),
        "probe failed"
    );
    assert_eq!(read_marker(&f.marker), Ok(Some(false)));
    assert_eq!(
        f.owner.act(right(), &before.id, None).await.err().unwrap(),
        "Observe the TV before an action"
    );
    assert_eq!(f.script.lock().unwrap().calls, ["capture", "probe"]);
}

#[tokio::test]
async fn failed_disconnect_retains_closing_and_never_claims_already_disconnected() {
    let f = fixture(vec![]);
    f.script.lock().unwrap().closes.push_back(IoStep::Error);
    assert_eq!(
        f.owner.disconnect().await,
        Err("cleanup unconfirmed".into())
    );
    assert!(matches!(&*f.owner.state.lock().await, State::Closing(_)));
    assert_eq!(
        f.owner.disconnect().await,
        Err("cleanup unconfirmed".into())
    );
    assert_eq!(
        f.owner
            .observe(ObserveOptions::default())
            .await
            .err()
            .unwrap(),
        "TV cleanup is incomplete"
    );
    assert_eq!(
        f.owner.act(right(), "old", None).await.err().unwrap(),
        "TV cleanup is incomplete"
    );
    assert_eq!(f.script.lock().unwrap().calls, ["close", "close"]);
}

async fn setup_in_progress(f: &Fixture) {
    let mut state = f.owner.state.lock().await;
    let State::Connected(s) = std::mem::replace(&mut *state, State::Disconnected) else {
        unreachable!()
    };
    *state = State::Closing(s);
}

#[tokio::test]
async fn failed_setup_with_confirmed_cleanup_returns_to_disconnected() {
    let f = fixture(vec![]);
    setup_in_progress(&f).await;
    let failed: Result<(), _> = Err("setup failed".into());
    assert_eq!(
        f.owner.state.lock().await.finish_setup(failed).await,
        Err("setup failed".into())
    );
    assert!(matches!(&*f.owner.state.lock().await, State::Disconnected));
    assert_eq!(f.owner.disconnect().await, Ok("TV is already disconnected"));
    assert_eq!(f.script.lock().unwrap().calls, ["close"]);
}

#[tokio::test]
async fn failed_setup_with_unconfirmed_cleanup_stays_closing() {
    let f = fixture(vec![]);
    setup_in_progress(&f).await;
    f.script.lock().unwrap().closes.push_back(IoStep::Error);
    let failed: Result<(), _> = Err("setup failed".into());
    assert_eq!(
        f.owner.state.lock().await.finish_setup(failed).await,
        Err("setup failed".into())
    );
    assert!(matches!(&*f.owner.state.lock().await, State::Closing(_)));
    assert_eq!(
        f.owner.disconnect().await,
        Err("cleanup unconfirmed".into())
    );
}

#[tokio::test]
async fn setup_cancelled_during_cleanup_stays_closing() {
    let f = fixture(vec![]);
    setup_in_progress(&f).await;
    f.script.lock().unwrap().closes.push_back(IoStep::Pending);
    let failed: Result<(), _> = Err("setup failed".into());
    let mut state = f.owner.state.lock().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(1), state.finish_setup(failed))
            .await
            .is_err()
    );
    assert!(matches!(&*state, State::Closing(_)));
}

#[tokio::test]
async fn successful_setup_becomes_connected_without_closing() {
    let f = fixture(vec![]);
    setup_in_progress(&f).await;
    assert_eq!(f.owner.state.lock().await.finish_setup(Ok(7)).await, Ok(7));
    assert!(matches!(&*f.owner.state.lock().await, State::Connected(_)));
    assert!(f.script.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn cancelled_disconnect_retains_closing_and_cleanup_uncertainty() {
    let f = fixture(vec![]);
    f.script.lock().unwrap().closes.push_back(IoStep::Pending);
    assert!(
        tokio::time::timeout(Duration::from_millis(1), f.owner.disconnect())
            .await
            .is_err()
    );
    assert!(matches!(&*f.owner.state.lock().await, State::Closing(_)));
    assert_eq!(
        f.owner.disconnect().await,
        Err("cleanup unconfirmed".into())
    );
    assert_eq!(f.script.lock().unwrap().calls, ["close", "close"]);
}

#[tokio::test]
async fn successful_disconnect_is_idempotent_and_drops_observation_authority() {
    let f = fixture(vec![CaptureStep::Frame(1)]);
    let before = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert_eq!(f.owner.disconnect().await, Ok("TV disconnected"));
    assert_eq!(f.owner.disconnect().await, Ok("TV is already disconnected"));
    assert_eq!(
        f.owner.act(right(), &before.id, None).await.err().unwrap(),
        "TV is disconnected"
    );
    assert_eq!(f.script.lock().unwrap().calls, ["capture", "close"]);
}

#[tokio::test]
async fn sampling_stops_on_visual_change_and_caps_unchanged_frames() {
    let f = fixture(vec![
        CaptureStep::Frame(1),
        CaptureStep::Frame(1),
        CaptureStep::Frame(2),
        CaptureStep::Frame(2),
        CaptureStep::Frame(2),
        CaptureStep::Frame(2),
        CaptureStep::Frame(2),
    ]);
    let initial = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert_eq!(initial.visual_change, VisualChange::First);
    let options = ObserveOptions {
        change_timeout: Duration::from_secs(5),
        ..ObserveOptions::default()
    };
    let changed = f.owner.observe(options).await.unwrap();
    assert_eq!(changed.samples, 2);
    assert_eq!(changed.visual_change, VisualChange::Changed);
    assert_eq!(changed.image.bytes, [2]);
    let unchanged = f.owner.observe(options).await.unwrap();
    assert_eq!(unchanged.samples, 4);
    assert_eq!(unchanged.visual_change, VisualChange::Unchanged);
    assert_eq!(f.script.lock().unwrap().calls, ["capture"; 7]);
}

#[tokio::test]
async fn black_frame_is_reported_without_claiming_visible_progress() {
    let f = fixture(vec![CaptureStep::Frame(1), CaptureStep::Black]);
    f.owner.observe(ObserveOptions::default()).await.unwrap();
    let black = f
        .owner
        .observe(ObserveOptions {
            change_timeout: Duration::from_secs(5),
            ..ObserveOptions::default()
        })
        .await
        .unwrap();
    assert_eq!(black.visual_change, VisualChange::Black);
    assert_eq!(black.samples, 1);
    assert!(black.image.all_black);
}

#[tokio::test]
async fn concurrent_calls_return_busy_instead_of_queueing_a_stale_action() {
    let f = fixture(vec![CaptureStep::Pending]);
    let owner = f.owner.clone();
    let capture = tokio::spawn(async move { owner.observe(ObserveOptions::default()).await });
    f.capture_started.notified().await;
    assert_eq!(
        f.owner.act(right(), "stale", None).await.err().unwrap(),
        "TV session is busy"
    );
    assert_eq!(f.owner.disconnect().await, Err("TV session is busy".into()));
    capture.abort();
    assert!(matches!(capture.await, Err(error) if error.is_cancelled()));
    assert_eq!(f.script.lock().unwrap().calls, ["capture"]);
    assert!(
        matches!(&*f.owner.state.lock().await, State::Connected(s) if s.view == View::NeedsObservation)
    );
}

#[tokio::test]
async fn change_window_does_not_start_an_extra_sample_after_its_deadline() {
    let f = fixture(vec![
        CaptureStep::Frame(1),
        CaptureStep::Frame(1),
        CaptureStep::Frame(1),
    ]);
    f.owner.observe(ObserveOptions::default()).await.unwrap();
    let observation = f
        .owner
        .observe(ObserveOptions {
            change_timeout: Duration::from_millis(50),
            ..ObserveOptions::default()
        })
        .await
        .unwrap();
    assert_eq!(observation.samples, 1);
    assert_eq!(observation.visual_change, VisualChange::Unchanged);
    assert_eq!(f.script.lock().unwrap().calls, ["capture", "capture"]);
}

#[tokio::test]
async fn previous_session_observation_cannot_authorize_an_action_after_reconnect() {
    let f = fixture(vec![CaptureStep::Frame(1), CaptureStep::Frame(1)]);
    let before = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert_eq!(f.owner.disconnect().await, Ok("TV disconnected"));
    *f.owner.state.lock().await = State::Connected(Box::new(Connected {
        _lock: File::create(f._directory.path().join("session.lock")).unwrap(),
        device: FakeDevice {
            script: f.script.clone(),
            marker: f.marker.clone(),
            capture_started: f.capture_started.clone(),
        },
        view: View::NeedsObservation,
        baseline: None,
        healthy: true,
        marker: f.marker.clone(),
    }));
    assert_eq!(
        f.owner.act(right(), &before.id, None).await.err().unwrap(),
        "Observe the TV before an action"
    );
    let after = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert_eq!(after.image.fingerprint, before.image.fingerprint);
    assert_ne!(after.id, before.id);
    assert_eq!(
        f.owner.act(right(), &before.id, None).await.err().unwrap(),
        "Observation ID is stale; observe before acting"
    );
    let report = f.owner.act(right(), &after.id, None).await.unwrap();
    assert_eq!(report.delivery, Delivery::Sent);
    assert_eq!(
        f.script.lock().unwrap().calls,
        ["capture", "close", "capture", "probe", "send"]
    );
    assert_eq!(f.script.lock().unwrap().buttons, [("RIGHT".into(), 1)]);
}

#[tokio::test]
async fn observation_keeps_recovery_evidence_across_later_changed_samples() {
    let f = fixture(vec![
        CaptureStep::Frame(1),
        CaptureStep::Recovered(1),
        CaptureStep::Frame(2),
        CaptureStep::Frame(3),
    ]);
    let initial = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert!(!initial.image.recovered);
    let changed = f
        .owner
        .observe(ObserveOptions {
            change_timeout: Duration::from_secs(5),
            ..ObserveOptions::default()
        })
        .await
        .unwrap();
    assert_eq!(changed.samples, 2);
    assert_eq!(changed.visual_change, VisualChange::Changed);
    assert_eq!(changed.image.bytes, [2]);
    assert!(changed.image.recovered);
    let next = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert!(!next.image.recovered);
    assert_eq!(next.image.bytes, [3]);
    assert_eq!(f.script.lock().unwrap().calls, ["capture"; 4]);
}

#[tokio::test]
async fn keepalive_contention_waits_without_queueing_another_user_action() {
    let f = fixture(vec![CaptureStep::Frame(1)]);
    let id = f.owner.observe(ObserveOptions::default()).await.unwrap().id;
    let heartbeat = f.owner.state.lock().await;
    let mut action = Box::pin(f.owner.act(right(), &id, None));
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut action)
            .await
            .is_err()
    );
    assert!(f.script.lock().unwrap().buttons.is_empty());
    assert_eq!(
        f.owner.act(right(), &id, None).await.err(),
        Some("TV session is busy".into())
    );
    drop(heartbeat);
    assert_eq!(action.await.unwrap().delivery, Delivery::Sent);
    assert_eq!(f.script.lock().unwrap().buttons.len(), 1);
    let heartbeat = f.owner.state.lock().await;
    let mut disconnect = Box::pin(f.owner.disconnect());
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut disconnect)
            .await
            .is_err()
    );
    drop(heartbeat);
    assert_eq!(disconnect.await.unwrap(), "TV disconnected");
}

#[tokio::test]
async fn waiting_for_keepalive_can_cancel_and_rechecks_health_before_send() {
    let f = fixture(vec![CaptureStep::Frame(1)]);
    let id = f.owner.observe(ObserveOptions::default()).await.unwrap().id;
    let mut heartbeat = f.owner.state.lock().await;
    let mut action = Box::pin(f.owner.act(right(), &id, None));
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut action)
            .await
            .is_err()
    );
    drop(action);
    assert_eq!(read_marker(&f.marker).unwrap(), Some(false));
    let State::Connected(s) = &mut *heartbeat else {
        panic!("connected")
    };
    assert_eq!(s.view, View::Fresh(id.clone()));
    let mut action = Box::pin(f.owner.act(right(), &id, None));
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut action)
            .await
            .is_err()
    );
    s.healthy = false;
    s.view = View::NeedsObservation;
    drop(heartbeat);
    assert!(action.await.is_err());
    assert!(f.script.lock().unwrap().buttons.is_empty());
    assert_eq!(read_marker(&f.marker).unwrap(), Some(false));
}

#[tokio::test]
async fn failed_marker_write_sends_nothing_and_does_not_claim_unresolved_delivery() {
    let f = fixture(vec![CaptureStep::Frame(1), CaptureStep::Frame(2)]);
    let before = f.owner.observe(ObserveOptions::default()).await.unwrap();
    let unwritable = f._directory.path().join("not-a-file");
    std::fs::create_dir(&unwritable).unwrap();
    {
        let mut state = f.owner.state.lock().await;
        let State::Connected(s) = &mut *state else {
            panic!("connected")
        };
        s.marker = unwritable;
    }
    assert_eq!(
        f.owner.act(right(), &before.id, None).await.err().unwrap(),
        "Could not save local action state"
    );
    assert_eq!(f.script.lock().unwrap().calls, ["capture", "probe"]);
    assert!(
        matches!(&*f.owner.state.lock().await, State::Connected(s) if s.view == View::NeedsObservation)
    );
    let after = f.owner.observe(ObserveOptions::default()).await.unwrap();
    assert!(!after.delivery_unresolved);
    assert!(f.script.lock().unwrap().buttons.is_empty());
}

#[test]
fn only_an_explicit_confirmation_clears_unresolved_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("action-state");
    write_marker(&path, true).unwrap();
    assert_eq!(
        acknowledge_marker(&path, true, || Ok(false)),
        Ok(Acknowledgement::Declined)
    );
    assert_eq!(read_marker(&path), Ok(Some(true)));
    assert_eq!(
        acknowledge_marker(&path, true, || Err("no terminal")),
        Err("no terminal")
    );
    assert_eq!(read_marker(&path), Ok(Some(true)));
    assert_eq!(
        acknowledge_marker(&path, true, || Ok(true)),
        Ok(Acknowledgement::Cleared)
    );
    assert_eq!(read_marker(&path), Ok(Some(false)));
    assert_eq!(
        acknowledge_marker(&path, false, || panic!("nothing to confirm")),
        Ok(Acknowledgement::NothingUnresolved)
    );
}
