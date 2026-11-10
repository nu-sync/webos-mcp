//! The MCP tool surface: schemas, input validation, result formatting, and
//! operation metrics. Generic over the device so the same handlers run against
//! the live TV and a simulated one.
use crate::{
    error::TvError,
    metrics::{Metrics, Operation, Outcome},
    session::{Action, ActionReport, Device, Observation, ObserveOptions, SessionOwner},
};
use base64::Engine;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The six MCP tools over one session. Every front end calls these handlers.
pub struct TvServer<D> {
    metrics: Metrics,
    session: SessionOwner<D>,
    /// Becomes true when the front end stops; calls in flight are then dropped.
    stopping: tokio::sync::watch::Receiver<bool>,
}

impl<D> Clone for TvServer<D> {
    fn clone(&self) -> Self {
        Self {
            metrics: self.metrics.clone(),
            session: self.session.clone(),
            stopping: self.stopping.clone(),
        }
    }
}

const STOPPING: &str = "The server is stopping; the call was cancelled";

impl<D: Device + 'static> TvServer<D> {
    /// Run session work unless the front end stops first. Dropping the work
    /// is the session's cancellation path: an action mid-send stays
    /// unresolved, and the request gate is released so shutdown can close the
    /// session. The MCP library does not drop handlers on its own when the
    /// client goes away.
    async fn unless_stopping<T>(&self, work: impl Future<Output = T>) -> Option<T> {
        let mut stopping = self.stopping.clone();
        tokio::select! {
            biased;
            _ = stopping.wait_for(|stopping| *stopping) => None,
            finished = work => Some(finished),
        }
    }
}

/// The result of an action cut short by shutdown. Whether it was sent is not
/// known at this layer, so it is never reported as safe to repeat.
fn action_stopped() -> CallToolResult {
    tool_result(
        json!({"delivery":"unknown", "action_error":STOPPING, "do_not_repeat_action":true}),
        None,
        true,
    )
}

fn stopped() -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(STOPPING)])
}

#[derive(Clone, Debug, Deserialize, JsonSchema, clap::ValueEnum)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[value(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Back,
    Home,
}

// The parameter types below define both the MCP input schemas and the CLI
// options. Help text lives in `arg` attributes because doc comments would be
// published as schema descriptions. Ranges are enforced by the handlers, so
// both front ends return the same validation errors.
#[derive(Debug, Deserialize, JsonSchema, clap::Args)]
#[serde(deny_unknown_fields)]
pub struct PressParams {
    #[arg(long, value_enum, help = "Remote button to press")]
    pub button: Button,
    #[arg(long, help = "ID from the latest observation")]
    pub observation_id: String,
    #[schemars(range(min = 1, max = 5))]
    #[arg(long, help = "Number of paced presses, 1 to 5 [default: 1]")]
    pub count: Option<u8>,
    #[arg(
        long,
        help = "Capture a new screenshot after the action [default: true]"
    )]
    pub observe: Option<bool>,
    #[schemars(range(min = 0, max = 5000))]
    #[arg(long, help = "Delay before capturing, 0 to 5000 [default: 250]")]
    pub wait_ms: Option<u64>,
    #[schemars(range(min = 0, max = 3000))]
    #[arg(
        long,
        help = "Window for sampling a pixel change, 0 to 3000 [default: 1000]"
    )]
    pub change_timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema, clap::Args)]
#[serde(deny_unknown_fields)]
pub struct LaunchParams {
    #[arg(long, help = "webOS app ID to launch")]
    pub app_id: String,
    #[arg(long, help = "ID from the latest observation")]
    pub observation_id: String,
    #[arg(
        long,
        help = "Capture a new screenshot after the action [default: true]"
    )]
    pub observe: Option<bool>,
    #[schemars(range(min = 0, max = 5000))]
    #[arg(long, help = "Delay before capturing, 0 to 5000 [default: 250]")]
    pub wait_ms: Option<u64>,
    #[schemars(range(min = 0, max = 3000))]
    #[arg(
        long,
        help = "Window for sampling a pixel change, 0 to 3000 [default: 1000]"
    )]
    pub change_timeout_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, clap::Args)]
#[serde(deny_unknown_fields)]
pub struct ObserveParams {
    #[schemars(range(min = 0, max = 5000))]
    #[arg(long, help = "Delay before capturing, 0 to 5000 [default: 0]")]
    pub wait_ms: Option<u64>,
    #[schemars(range(min = 0, max = 3000))]
    #[arg(
        long,
        help = "Window for sampling a pixel change, 0 to 3000 [default: 0]"
    )]
    pub change_timeout_ms: Option<u64>,
}

fn observation_options(
    wait: Option<u64>,
    timeout: Option<u64>,
    after_action: bool,
) -> Result<ObserveOptions, &'static str> {
    let wait = wait.unwrap_or(if after_action { 250 } else { 0 });
    let timeout = timeout.unwrap_or(if after_action { 1000 } else { 0 });
    if wait > 5000 {
        return Err("wait_ms must be between 0 and 5000.");
    }
    if timeout > 3000 {
        return Err("change_timeout_ms must be between 0 and 3000.");
    }
    Ok(ObserveOptions {
        wait: Duration::from_millis(wait),
        change_timeout: Duration::from_millis(timeout),
    })
}

/// Encoded image bytes and their MIME type, as captured.
type Image = (Vec<u8>, &'static str);

fn tool_result(metadata: Value, image: Option<Image>, failed: bool) -> CallToolResult {
    let mut content = Vec::new();
    if let Some((bytes, mime_type)) = image {
        content.push(ContentBlock::image(
            base64::engine::general_purpose::STANDARD.encode(bytes),
            mime_type,
        ));
    }
    content.push(ContentBlock::text(metadata.to_string()));
    let mut result = if failed {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    };
    result.structured_content = Some(metadata);
    result
}

fn observation_content(observation: Observation) -> (Value, Image) {
    let metadata = json!({
        "observation_id": observation.id,
        "visual_change": observation.visual_change,
        "delivery_unresolved": observation.delivery_unresolved,
        "all_black": observation.image.all_black,
        "width": observation.image.width,
        "height": observation.image.height,
        "capture_recovered": observation.image.recovered,
        "ui_settled": "unknown",
        "samples": observation.samples,
        "elapsed_ms": observation.elapsed_ms,
        "returned_at_unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
    });
    (
        metadata,
        (observation.image.bytes, observation.image.mime_type),
    )
}

fn action_result(report: ActionReport) -> (CallToolResult, Outcome) {
    let (mut metadata, image) = match report.observation {
        Some(observation) => {
            let (metadata, image) = observation_content(observation);
            (metadata, Some(image))
        }
        None => (json!({}), None),
    };
    metadata["delivery"] = json!(report.delivery);
    metadata["action_error"] = json!(report.error.map(TvError::message));
    metadata["observation_error"] = json!(report.observation_error.map(TvError::message));
    metadata["do_not_repeat_action"] = json!(true);
    let failed = report.error.is_some()
        || report.observation_error.is_some()
        || metadata["delivery"] == "unknown";
    let outcome = if failed {
        if report
            .error
            .into_iter()
            .chain(report.observation_error)
            .any(|error| error.outcome() == Outcome::Timeout)
        {
            Outcome::Timeout
        } else {
            Outcome::Failure
        }
    } else {
        Outcome::Success
    };
    (tool_result(metadata, image, failed), outcome)
}

fn action_rejected(error: impl Into<TvError>) -> CallToolResult {
    let message = error.into().message();
    tool_result(
        json!({"delivery":"not_sent", "action_error":message, "do_not_repeat_action":false}),
        None,
        true,
    )
}

fn message_result(value: Result<&'static str, TvError>) -> (CallToolResult, Outcome) {
    match value {
        Ok(message) => (
            CallToolResult::success(vec![ContentBlock::text(message)]),
            Outcome::Success,
        ),
        Err(error) => (
            CallToolResult::error(vec![ContentBlock::text(error.message())]),
            error.outcome(),
        ),
    }
}

#[tool_router(vis = "pub")]
impl<D: Device + 'static> TvServer<D> {
    #[tool(description = "Connect to the configured TV without waking it.")]
    pub async fn tv_connect(&self) -> CallToolResult {
        let measurement = self.metrics.begin(Operation::Connect);
        let Some(connected) = self.unless_stopping(self.session.connect()).await else {
            return stopped();
        };
        let (result, outcome) = message_result(connected);
        measurement.finish(outcome);
        result
    }

    #[tool(
        description = "Capture a fresh TV image (JPEG by default) and observation ID, optionally waiting for pixel changes. A change or delay does not prove the UI settled."
    )]
    pub async fn tv_observe(
        &self,
        Parameters(params): Parameters<ObserveParams>,
    ) -> CallToolResult {
        let measurement = self.metrics.begin(Operation::Observe);
        let options = match observation_options(params.wait_ms, params.change_timeout_ms, false) {
            Ok(options) => options,
            Err(error) => {
                measurement.finish(Outcome::Failure);
                return tool_result(json!({"observation_error":error}), None, true);
            }
        };
        let Some(observed) = self.unless_stopping(self.session.observe(options)).await else {
            return tool_result(json!({"observation_error":STOPPING}), None, true);
        };
        match observed {
            Ok(observation) => {
                let (metadata, image) = observation_content(observation);
                measurement.finish(Outcome::Success);
                tool_result(metadata, Some(image), false)
            }
            Err(error) => {
                measurement.finish(error.outcome());
                tool_result(json!({"observation_error":error.message()}), None, true)
            }
        }
    }

    #[tool(
        description = "Send one to five paced button presses using the latest observation ID. Returns a new screenshot by default. Never repeat an action merely because its screenshot failed."
    )]
    pub async fn tv_press(&self, Parameters(params): Parameters<PressParams>) -> CallToolResult {
        let measurement = self.metrics.begin(Operation::Press);
        let options = match observation_options(params.wait_ms, params.change_timeout_ms, true) {
            Ok(options) => options,
            Err(error) => {
                measurement.finish(Outcome::Failure);
                return action_rejected(error);
            }
        };
        if !matches!(params.count.unwrap_or(1), 1..=5) {
            measurement.finish(Outcome::Failure);
            return action_rejected("count must be between 1 and 5.");
        }
        if params.observation_id.trim().is_empty() {
            measurement.finish(Outcome::Failure);
            return action_rejected("observation_id must not be empty.");
        }
        let button = match params.button {
            Button::Up => "UP",
            Button::Down => "DOWN",
            Button::Left => "LEFT",
            Button::Right => "RIGHT",
            Button::Enter => "ENTER",
            Button::Back => "BACK",
            Button::Home => "HOME",
        };
        let Some(response) = self
            .unless_stopping(self.session.act(
                Action::Press {
                    button: button.into(),
                    count: params.count.unwrap_or(1),
                },
                &params.observation_id,
                params.observe.unwrap_or(true).then_some(options),
            ))
            .await
        else {
            return action_stopped();
        };
        let (result, outcome) = match response {
            Ok(report) => action_result(report),
            Err(error) => (action_rejected(error), error.outcome()),
        };
        measurement.finish(outcome);
        result
    }

    #[tool(
        description = "Launch an app using the latest observation ID; returns a new screenshot by default. Local send completion and visual confirmation are separate results."
    )]
    pub async fn tv_launch(&self, Parameters(params): Parameters<LaunchParams>) -> CallToolResult {
        let measurement = self.metrics.begin(Operation::Launch);
        let options = match observation_options(params.wait_ms, params.change_timeout_ms, true) {
            Ok(options) => options,
            Err(error) => {
                measurement.finish(Outcome::Failure);
                return action_rejected(error);
            }
        };
        if params.app_id.trim().is_empty() {
            measurement.finish(Outcome::Failure);
            return action_rejected("app_id must not be empty.");
        }
        if params.observation_id.trim().is_empty() {
            measurement.finish(Outcome::Failure);
            return action_rejected("observation_id must not be empty.");
        }
        let Some(response) = self
            .unless_stopping(self.session.act(
                Action::Launch {
                    app_id: params.app_id,
                },
                &params.observation_id,
                params.observe.unwrap_or(true).then_some(options),
            ))
            .await
        else {
            return action_stopped();
        };
        let (result, outcome) = match response {
            Ok(report) => action_result(report),
            Err(error) => (action_rejected(error), error.outcome()),
        };
        measurement.finish(outcome);
        result
    }

    #[tool(description = "Close the TV session, reporting any unconfirmed cleanup.")]
    pub async fn tv_disconnect(&self) -> CallToolResult {
        let measurement = self.metrics.begin(Operation::Disconnect);
        let Some(disconnected) = self.unless_stopping(self.session.disconnect()).await else {
            return stopped();
        };
        let (result, outcome) = message_result(disconnected);
        measurement.finish(outcome);
        result
    }

    #[tool(
        description = "Read local aggregate timings and outcomes without contacting the TV. No screen, device, or request data is collected."
    )]
    pub fn tv_metrics(&self) -> CallToolResult {
        CallToolResult::structured(json!(self.metrics.snapshot()))
    }
}

/// A running front end: the tool server, its session, and the keepalive.
/// MCP stdio and the command line both start and stop through this type, so
/// they share one lifecycle as well as one set of handlers.
pub struct Front<D> {
    pub server: TvServer<D>,
    session: SessionOwner<D>,
    keepalive: tokio::task::JoinHandle<()>,
    stopping: tokio::sync::watch::Sender<bool>,
}

impl<D: Device + 'static> Front<D> {
    pub fn start(metrics: Metrics, session: SessionOwner<D>) -> Self {
        let (stopping, stopped) = tokio::sync::watch::channel(false);
        Self {
            keepalive: session.start_keepalive(),
            server: TvServer {
                metrics,
                session: session.clone(),
                stopping: stopped,
            },
            session,
            stopping,
        }
    }

    /// Release the session. Returns whether cleanup was confirmed.
    pub async fn stop(self) -> bool {
        self.keepalive.abort();
        let _ = self.keepalive.await;
        // Cancel any call still in flight, then wait for it to release the session.
        self.stopping.send_replace(true);
        self.session.shutdown().await.is_ok()
    }

    /// Serve MCP until the client closes the transport or `interrupt`
    /// resolves, then release the session. Returns whether cleanup was
    /// confirmed.
    pub async fn serve<T, E, A>(
        self,
        transport: T,
        interrupt: impl Future<Output = ()>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>
    where
        T: rmcp::transport::IntoTransport<rmcp::RoleServer, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let served = async {
            tokio::pin!(interrupt);
            // An interrupt during the handshake stops the server as well.
            let running = tokio::select! {
                running = self.server.clone().serve(transport) => running?,
                () = &mut interrupt => return Ok(()),
            };
            let cancel = running.cancellation_token();
            let waiting = running.waiting();
            tokio::pin!(waiting);
            tokio::select! {
                finished = &mut waiting => finished.map(drop),
                () = &mut interrupt => {
                    // Stop serving first so no new call starts during cleanup.
                    cancel.cancel();
                    waiting.await.map(drop)
                }
            }
            .map_err(Box::<dyn std::error::Error + Send + Sync>::from)
        }
        .await;
        let cleaned = self.stop().await;
        served.map(|()| cleaned)
    }
}

#[tool_handler]
impl<D: Device + 'static> ServerHandler for TvServer<D> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Delivery, VisualChange};

    #[test]
    fn sent_action_with_failed_capture_preserves_delivery_and_returns_no_image() {
        let (result, outcome) = action_result(ActionReport {
            delivery: Delivery::Sent,
            error: None,
            observation: None,
            observation_error: Some(TvError::timeout("Capture timed out")),
        });
        assert_eq!(result.is_error, Some(true));
        assert_eq!(outcome, Outcome::Timeout);
        assert!(
            result
                .content
                .iter()
                .all(|block| block.as_image().is_none())
        );
        let metadata = result.structured_content.unwrap();
        assert_eq!(metadata["delivery"], "sent");
        assert_eq!(metadata["action_error"], Value::Null);
        assert_eq!(metadata["observation_error"], "Capture timed out");
        assert_eq!(metadata["do_not_repeat_action"], true);
        assert!(metadata.get("observation_id").is_none());
    }

    #[test]
    fn uncertain_action_cannot_be_rendered_as_a_safe_rejection() {
        let (result, outcome) = action_result(ActionReport {
            delivery: Delivery::Unknown,
            error: Some(TvError::failure("Button delivery is uncertain")),
            observation: None,
            observation_error: None,
        });
        assert_eq!(result.is_error, Some(true));
        assert_eq!(outcome, Outcome::Failure);
        let metadata = result.structured_content.unwrap();
        assert_eq!(metadata["delivery"], "unknown");
        assert_eq!(metadata["do_not_repeat_action"], true);
        assert!(metadata.get("observation_id").is_none());
    }

    #[test]
    fn successful_combined_result_returns_the_new_frame_and_its_observation_id() {
        let (result, outcome) = action_result(ActionReport {
            delivery: Delivery::Sent,
            error: None,
            observation: Some(Observation {
                image: crate::capture::CapturedImage {
                    bytes: vec![1, 2, 3],
                    mime_type: "image/jpeg",
                    all_black: false,
                    fingerprint: [0; 32],
                    width: 1,
                    height: 1,
                    recovered: true,
                },
                id: "synthetic-session-2".into(),
                visual_change: VisualChange::Changed,
                delivery_unresolved: false,
                samples: 2,
                elapsed_ms: 300,
            }),
            observation_error: None,
        });
        assert_eq!(outcome, Outcome::Success);
        assert_ne!(result.is_error, Some(true));
        let image = result.content[0].as_image().unwrap();
        assert_eq!(image.data, "AQID");
        assert_eq!(image.mime_type, "image/jpeg");
        let metadata = result.structured_content.unwrap();
        assert_eq!(metadata["observation_id"], "synthetic-session-2");
        assert_eq!(metadata["visual_change"], "changed");
        assert_eq!(metadata["delivery"], "sent");
        assert_eq!(metadata["ui_settled"], "unknown");
        assert_eq!(metadata["capture_recovered"], true);
        assert_eq!(metadata["samples"], 2);
        assert_eq!(metadata["elapsed_ms"], 300);
    }
}
