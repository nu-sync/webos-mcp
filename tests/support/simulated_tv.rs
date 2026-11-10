//! A simulated TV for end-to-end tests. It is not a model of webOS: it keeps a
//! cursor and a current app, draws them into a small real PNG, and records
//! every input it receives so two runs can be compared.
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use webos_mcp::{
    capture::CapturedImage,
    error::TvError,
    metrics::Metrics,
    session::{Action, Device},
};

/// Launching this app makes the next capture fail.
pub const FAIL_NEXT_CAPTURE: &str = "sim.fail-next-capture";
/// Launching this app makes the next capture never return.
pub const HANG_NEXT_CAPTURE: &str = "sim.hang-next-capture";
/// Pressing this button fails with delivery unknown.
pub const UNCERTAIN_BUTTON: &str = "BACK";
/// Pressing this button never returns from the send.
pub const HANGING_BUTTON: &str = "ENTER";

#[derive(Default)]
pub struct Tv {
    /// Everything the TV was asked to do, in order.
    pub events: Vec<String>,
    cursor: u8,
    app: String,
    fail_next_capture: bool,
    hang_next_capture: bool,
}

pub type Shared = Arc<Mutex<Tv>>;

pub struct SimulatedTv(Shared);

/// A device factory for `SessionOwner::new` and the TV it connects to. The TV
/// outlives sessions, as a real one does.
pub fn simulated_tv() -> (impl Fn() -> SimulatedTv + Send + Sync + 'static, Shared) {
    let tv = Shared::default();
    let handle = tv.clone();
    (move || SimulatedTv(tv.clone()), handle)
}

fn frame(cursor: u8, app: &str) -> CapturedImage {
    let (width, height) = (4, 2);
    let tint = app.bytes().fold(40u8, u8::wrapping_add) | 1;
    let pixels: Vec<u8> = (0..width * height)
        .flat_map(|position| {
            if position == u32::from(cursor) % (width * height) {
                [255, 255, 255]
            } else {
                [tint, 20, 60]
            }
        })
        .collect();
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&pixels).unwrap();
        writer.finish().unwrap();
    }
    CapturedImage {
        bytes,
        mime_type: "image/png",
        all_black: false,
        fingerprint: Sha256::digest(&pixels).into(),
        width,
        height,
        recovered: false,
    }
}

impl Device for SimulatedTv {
    async fn establish(&mut self, _: &Metrics) -> Result<bool, TvError> {
        self.0.lock().unwrap().events.push("establish".into());
        Ok(true)
    }

    async fn probe(&mut self) -> Result<(), TvError> {
        Ok(())
    }

    async fn send(&mut self, action: &Action) -> Result<(), TvError> {
        let hang = {
            let mut tv = self.0.lock().unwrap();
            match action {
                Action::Press { button, count } => {
                    tv.events.push(format!("press {button} x{count}"));
                    if button == UNCERTAIN_BUTTON {
                        return Err(TvError::timeout("Button delivery is uncertain"));
                    }
                    match button.as_str() {
                        "RIGHT" | "DOWN" => tv.cursor = tv.cursor.wrapping_add(*count),
                        "LEFT" | "UP" => tv.cursor = tv.cursor.wrapping_sub(*count),
                        "HOME" => (tv.cursor, tv.app) = (0, String::new()),
                        _ => {}
                    }
                    button == HANGING_BUTTON
                }
                Action::Launch { app_id } => {
                    tv.events.push(format!("launch {app_id}"));
                    tv.fail_next_capture = app_id == FAIL_NEXT_CAPTURE;
                    tv.hang_next_capture = app_id == HANG_NEXT_CAPTURE;
                    tv.app.clone_from(app_id);
                    false
                }
            }
        };
        if hang {
            std::future::pending::<()>().await;
        }
        Ok(())
    }

    async fn capture(&mut self) -> Result<CapturedImage, TvError> {
        let (hang, image) = {
            let mut tv = self.0.lock().unwrap();
            if std::mem::take(&mut tv.fail_next_capture) {
                tv.events.push("capture failed".into());
                return Err(TvError::failure("Capture transfer failed"));
            }
            let hang = std::mem::take(&mut tv.hang_next_capture);
            tv.events
                .push(if hang { "capture hung" } else { "capture" }.into());
            (hang, frame(tv.cursor, &tv.app))
        };
        if hang {
            std::future::pending::<()>().await;
        }
        Ok(image)
    }

    async fn close(&mut self) -> Result<(), TvError> {
        self.0.lock().unwrap().events.push("close".into());
        Ok(())
    }
}
