use super::*;
use std::{cell::RefCell, collections::VecDeque, sync::Mutex};

enum CaptureStep {
    Frame,
    Error(CaptureError),
    Pending,
}
enum CloseStep {
    Ok,
    Error,
    Pending,
}
#[derive(Default)]
struct Script {
    capture: VecDeque<CaptureStep>,
    close: VecDeque<CloseStep>,
    calls: Vec<&'static str>,
    reject_connect: bool,
    pending_connect: bool,
}
thread_local! {
    static SCRIPT: RefCell<Option<Arc<Mutex<Script>>>> = const { RefCell::new(None) };
}
struct FakeConnection(Arc<Mutex<Script>>);
impl CaptureConnection for FakeConnection {
    async fn connect(_: &CaptureConfig) -> Result<Self, CaptureError> {
        let script = SCRIPT.with(|slot| slot.borrow().as_ref().unwrap().clone());
        let pending = {
            let mut state = script.lock().unwrap();
            state.calls.push("connect");
            if state.reject_connect {
                return Err(CaptureError::Connection);
            }
            state.pending_connect
        };
        if pending {
            return std::future::pending().await;
        }
        Ok(Self(script))
    }
    async fn capture(&mut self) -> Result<CapturedImage, CaptureError> {
        let step = {
            let mut state = self.0.lock().unwrap();
            state.calls.push("capture");
            state.capture.pop_front().expect("unexpected capture")
        };
        match step {
            CaptureStep::Frame => decode_image(png(
                1,
                1,
                png::ColorType::Rgb,
                png::BitDepth::Eight,
                &[1, 2, 3],
            )),
            CaptureStep::Error(error) => Err(error),
            CaptureStep::Pending => std::future::pending().await,
        }
    }
    async fn close(self) -> Result<(), CaptureError> {
        let step = {
            let mut state = self.0.lock().unwrap();
            state.calls.push("close");
            state.close.pop_front().unwrap_or(CloseStep::Ok)
        };
        match step {
            CloseStep::Ok => Ok(()),
            CloseStep::Error => Err(CaptureError::CleanupUnconfirmed),
            CloseStep::Pending => std::future::pending().await,
        }
    }
}
fn config() -> CaptureConfig {
    CaptureConfig {
        endpoint: "192.0.2.1:22".parse().unwrap(),
        username: "test".into(),
        agent_socket: "synthetic".into(),
        public_key: "synthetic".into(),
    }
}
async fn fake(
    capture: Vec<CaptureStep>,
    close: Vec<CloseStep>,
) -> (Recovery<FakeConnection>, Arc<Mutex<Script>>) {
    let script = Arc::new(Mutex::new(Script {
        capture: capture.into(),
        close: close.into(),
        ..Default::default()
    }));
    SCRIPT.with(|slot| *slot.borrow_mut() = Some(script.clone()));
    (Recovery::connect(config()).await.unwrap(), script)
}
fn png(
    width: u32,
    height: u32,
    color: png::ColorType,
    depth: png::BitDepth,
    bytes: &[u8],
) -> Vec<u8> {
    let mut image = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut image, width, height);
        encoder.set_color(color);
        encoder.set_depth(depth);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(bytes).unwrap();
        writer.finish().unwrap();
    }
    image
}

#[tokio::test]
async fn channel_open_failure_closes_and_reconnects_once() {
    let (mut recovery, script) = fake(
        vec![
            CaptureStep::Error(CaptureError::ChannelOpen),
            CaptureStep::Frame,
        ],
        vec![],
    )
    .await;
    let frame = recovery.capture().await.unwrap();
    assert_eq!((frame.width, frame.height, frame.all_black), (1, 1, false));
    assert!(frame.recovered);
    recovery.close().await.unwrap();
    assert_eq!(
        script.lock().unwrap().calls,
        ["connect", "capture", "close", "connect", "capture", "close"]
    );
}

#[tokio::test]
async fn repeated_channel_failure_has_a_bounded_retry_budget() {
    let (mut recovery, script) = fake(
        vec![
            CaptureStep::Error(CaptureError::ChannelOpen),
            CaptureStep::Error(CaptureError::ChannelOpen),
        ],
        vec![],
    )
    .await;
    assert!(matches!(
        recovery.capture().await,
        Err(CaptureError::ChannelOpen)
    ));
    assert_eq!(
        script.lock().unwrap().calls,
        ["connect", "capture", "close", "connect", "capture", "close"]
    );
    recovery.close().await.unwrap();
}

#[tokio::test]
async fn transport_failures_never_replay_capture_with_unknown_remote_cleanup() {
    for error in [
        CaptureError::Request,
        CaptureError::Transfer,
        CaptureError::Timeout,
        CaptureError::TransferLimit,
    ] {
        let (mut recovery, script) = fake(vec![CaptureStep::Error(error)], vec![]).await;
        assert!(matches!(recovery.capture().await, Err(actual) if actual == error));
        assert!(matches!(
            recovery.capture().await,
            Err(CaptureError::CleanupUnconfirmed)
        ));
        assert_eq!(
            recovery.close().await,
            Err(CaptureError::CleanupUnconfirmed)
        );
        assert_eq!(
            script.lock().unwrap().calls,
            ["connect", "capture", "close"]
        );
    }
}

#[tokio::test]
async fn invalid_image_does_not_discard_a_clean_connection_or_return_an_old_frame() {
    let (mut recovery, script) = fake(
        vec![
            CaptureStep::Frame,
            CaptureStep::Error(CaptureError::InvalidImage),
            CaptureStep::Frame,
        ],
        vec![],
    )
    .await;
    recovery.capture().await.unwrap();
    assert!(matches!(
        recovery.capture().await,
        Err(CaptureError::InvalidImage)
    ));
    let fresh = recovery.capture().await.unwrap();
    assert_eq!((fresh.width, fresh.height), (1, 1));
    assert_eq!(
        script.lock().unwrap().calls,
        ["connect", "capture", "capture", "capture"]
    );
}

#[tokio::test]
async fn failed_close_blocks_reconnect_and_remains_visible() {
    let (mut recovery, script) = fake(
        vec![CaptureStep::Error(CaptureError::ChannelOpen)],
        vec![CloseStep::Error],
    )
    .await;
    assert!(matches!(
        recovery.capture().await,
        Err(CaptureError::CleanupUnconfirmed)
    ));
    assert_eq!(
        recovery.close().await,
        Err(CaptureError::CleanupUnconfirmed)
    );
    assert_eq!(
        script.lock().unwrap().calls,
        ["connect", "capture", "close"]
    );
}

#[tokio::test]
async fn cancellation_during_capture_keeps_cleanup_uncertain() {
    let (mut recovery, script) = fake(vec![CaptureStep::Pending], vec![]).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(1), recovery.capture())
            .await
            .is_err()
    );
    assert!(matches!(
        recovery.capture().await,
        Err(CaptureError::CleanupUnconfirmed)
    ));
    assert_eq!(
        recovery.close().await,
        Err(CaptureError::CleanupUnconfirmed)
    );
    assert_eq!(script.lock().unwrap().calls, ["connect", "capture"]);
}

#[tokio::test]
async fn cancellation_during_close_keeps_cleanup_uncertain() {
    let (mut recovery, script) = fake(vec![], vec![CloseStep::Pending]).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(1), recovery.close())
            .await
            .is_err()
    );
    assert_eq!(
        recovery.close().await,
        Err(CaptureError::CleanupUnconfirmed)
    );
    assert!(matches!(
        recovery.capture().await,
        Err(CaptureError::CleanupUnconfirmed)
    ));
    assert_eq!(script.lock().unwrap().calls, ["connect", "close"]);
}

#[tokio::test]
async fn cancelled_recovery_close_does_not_reauthenticate() {
    let (mut recovery, script) = fake(
        vec![CaptureStep::Error(CaptureError::ChannelOpen)],
        vec![CloseStep::Pending],
    )
    .await;
    assert!(
        tokio::time::timeout(Duration::from_millis(1), recovery.capture())
            .await
            .is_err()
    );
    assert!(matches!(
        recovery.capture().await,
        Err(CaptureError::CleanupUnconfirmed)
    ));
    assert_eq!(
        script.lock().unwrap().calls,
        ["connect", "capture", "close"]
    );
}

#[tokio::test]
async fn close_is_idempotent_after_success_and_capture_is_rejected() {
    let (mut recovery, script) = fake(vec![], vec![]).await;
    assert_eq!(recovery.close().await, Ok(()));
    assert_eq!(recovery.close().await, Ok(()));
    assert!(matches!(
        recovery.capture().await,
        Err(CaptureError::Closed)
    ));
    assert_eq!(script.lock().unwrap().calls, ["connect", "close"]);
}

#[tokio::test]
async fn close_waits_and_reports_its_timeout() {
    let (finished, wait) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(confirm_ssh_close(Duration::from_secs(1), async {
        wait.await.map_err(|_| "SSH cleanup could not be confirmed")
    }));
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    finished.send(()).unwrap();
    assert_eq!(task.await.unwrap(), Ok(()));
    assert_eq!(
        confirm_ssh_close(Duration::from_millis(1), std::future::pending()).await,
        Err("SSH cleanup could not be confirmed")
    );
}

#[test]
fn validates_full_png_and_preserves_low_bits_of_sixteen_bit_color() {
    let image = png(
        1,
        1,
        png::ColorType::Rgb,
        png::BitDepth::Sixteen,
        &[0, 1, 0, 0, 0, 0],
    );
    let decoded = decode_image(image.clone()).unwrap();
    assert!(!decoded.all_black);
    assert_eq!(decoded.mime_type, "image/png");
    assert_eq!(decoded.bytes, image);
    assert!(matches!(
        decode_image(image[..image.len() - 8].to_vec()),
        Err(CaptureError::InvalidImage)
    ));
    assert!(matches!(
        decode_image(b"not png".to_vec()),
        Err(CaptureError::InvalidImage)
    ));
    let black = png(
        1,
        1,
        png::ColorType::Rgba,
        png::BitDepth::Eight,
        &[0, 0, 0, 255],
    );
    assert!(decode_image(black).unwrap().all_black);
}

#[test]
fn jpeg_capture_is_validated_and_returned_without_reencoding() {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode(&[255, 0, 0], 1, 1, image::ExtendedColorType::Rgb8)
        .unwrap();
    let frame = decode_image(jpeg.clone()).unwrap();
    assert_eq!((frame.width, frame.height, frame.all_black), (1, 1, false));
    assert_eq!(frame.mime_type, "image/jpeg");
    assert_eq!(frame.bytes, jpeg);
    let mut black = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut black)
        .encode(&[0; 12], 2, 2, image::ExtendedColorType::Rgb8)
        .unwrap();
    let black = decode_image(black).unwrap();
    assert!(black.all_black);
    assert_ne!(black.fingerprint, frame.fingerprint);
    assert!(matches!(
        decode_image(jpeg[..jpeg.len() - 2].to_vec()),
        Err(CaptureError::InvalidImage)
    ));
}

#[test]
fn fingerprints_pixels_and_dimensions_not_png_metadata() {
    let plain = png(1, 1, png::ColorType::Rgb, png::BitDepth::Eight, &[1, 2, 3]);
    let mut metadata = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut metadata, 1, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder
            .add_text_chunk("note".into(), "synthetic metadata".into())
            .unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[1, 2, 3]).unwrap();
        writer.finish().unwrap();
    }
    assert_ne!(plain, metadata);
    assert_eq!(
        decode_image(plain).unwrap().fingerprint,
        decode_image(metadata).unwrap().fingerprint
    );
    let wide = png(
        2,
        1,
        png::ColorType::Rgb,
        png::BitDepth::Eight,
        &[1, 2, 3, 1, 2, 3],
    );
    let tall = png(
        1,
        2,
        png::ColorType::Rgb,
        png::BitDepth::Eight,
        &[1, 2, 3, 1, 2, 3],
    );
    assert_ne!(
        decode_image(wide).unwrap().fingerprint,
        decode_image(tall).unwrap().fingerprint
    );
}

#[tokio::test]
async fn cancelled_reauthentication_keeps_cleanup_uncertain() {
    let (mut recovery, script) =
        fake(vec![CaptureStep::Error(CaptureError::ChannelOpen)], vec![]).await;
    script.lock().unwrap().pending_connect = true;
    assert!(
        tokio::time::timeout(Duration::from_millis(1), recovery.capture())
            .await
            .is_err()
    );
    assert_eq!(
        recovery.close().await,
        Err(CaptureError::CleanupUnconfirmed)
    );
    assert!(matches!(
        recovery.capture().await,
        Err(CaptureError::CleanupUnconfirmed)
    ));
    assert_eq!(
        script.lock().unwrap().calls,
        ["connect", "capture", "close", "connect"]
    );
}

#[tokio::test]
async fn reauthentication_failure_does_not_capture_or_loop() {
    let (mut recovery, script) =
        fake(vec![CaptureStep::Error(CaptureError::ChannelOpen)], vec![]).await;
    script.lock().unwrap().reject_connect = true;
    assert!(matches!(
        recovery.capture().await,
        Err(CaptureError::Connection)
    ));
    assert_eq!(recovery.close().await, Ok(()));
    assert_eq!(
        script.lock().unwrap().calls,
        ["connect", "capture", "close", "connect"]
    );
}

#[tokio::test]
async fn failed_authentication_closes_existing_connection_before_reporting_retriable_failure() {
    let (recovery, script) = fake(vec![], vec![]).await;
    let CaptureState::Connected(connection) = recovery.state else {
        panic!("connected fixture")
    };
    assert!(matches!(
        finish_authentication(connection, false).await,
        Err(CaptureError::Connection)
    ));
    assert_eq!(script.lock().unwrap().calls, ["connect", "close"]);
}

#[tokio::test]
async fn authentication_failure_does_not_hide_unconfirmed_cleanup() {
    let (recovery, script) = fake(vec![], vec![CloseStep::Error]).await;
    let CaptureState::Connected(connection) = recovery.state else {
        panic!("connected fixture")
    };
    assert!(matches!(
        finish_authentication(connection, false).await,
        Err(CaptureError::CleanupUnconfirmed)
    ));
    assert_eq!(script.lock().unwrap().calls, ["connect", "close"]);
}

#[test]
fn agent_candidates_prefer_the_configured_path_then_the_environment() {
    let both = agent_candidates(" /configured ", Some("/environment".into()));
    assert_eq!(both[..2], ["/configured", "/environment"]);
    let unset = agent_candidates("", Some("/environment".into()));
    assert_eq!(unset[0], "/environment");
    let same = agent_candidates("/same", Some("/same".into()));
    assert_eq!(same.iter().filter(|path| *path == "/same").count(), 1);
    #[cfg(not(windows))]
    assert!(agent_candidates(" ", None).is_empty());
}

#[tokio::test]
async fn a_stale_agent_path_falls_back_to_the_next_candidate_once() {
    let tried = RefCell::new(Vec::new());
    let agent = connect_agent(
        vec!["stale".into(), "live".into(), "unused".into()],
        async |path| {
            tried.borrow_mut().push(path.clone());
            (path == "live").then_some(path)
        },
    )
    .await;
    assert_eq!(agent, Ok("live".to_owned()));
    assert_eq!(*tried.borrow(), ["stale", "live"]);
    let none = connect_agent(vec!["stale".into()], async |_| None::<()>).await;
    assert_eq!(none, Err(CaptureError::Connection));
}

#[test]
fn capture_timeout_is_typed_as_a_timeout_outcome() {
    use crate::metrics::Outcome;
    assert_eq!(
        TvError::from(CaptureError::Timeout).outcome(),
        Outcome::Timeout
    );
    assert_eq!(
        TvError::from(CaptureError::Transfer).outcome(),
        Outcome::Failure
    );
    assert_eq!(
        TvError::from(CaptureError::Timeout).message(),
        CaptureError::Timeout.message()
    );
}
