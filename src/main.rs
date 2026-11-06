mod local_store;
mod session;
mod setup;

use base64::Engine;
use rmcp::{
    ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    tool, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use session::{Action, ActionReport, Observation, ObserveOptions};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use webos_mcp::metrics::{Metrics, Operation, Outcome};

#[derive(Clone)]
struct TvServer {
    metrics: Metrics,
    session: session::SessionOwner,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum Button {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Back,
    Home,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PressParams {
    button: Button,
    observation_id: String,
    #[schemars(range(min = 1, max = 5))]
    count: Option<u8>,
    observe: Option<bool>,
    #[schemars(range(min = 0, max = 5000))]
    wait_ms: Option<u64>,
    #[schemars(range(min = 0, max = 3000))]
    change_timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LaunchParams {
    app_id: String,
    observation_id: String,
    observe: Option<bool>,
    #[schemars(range(min = 0, max = 5000))]
    wait_ms: Option<u64>,
    #[schemars(range(min = 0, max = 3000))]
    change_timeout_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ObserveParams {
    #[schemars(range(min = 0, max = 5000))]
    wait_ms: Option<u64>,
    #[schemars(range(min = 0, max = 3000))]
    change_timeout_ms: Option<u64>,
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

fn error_outcome(message: &str) -> Outcome {
    if message.contains("timed out") || message.contains("timeout") {
        Outcome::Timeout
    } else {
        Outcome::Failure
    }
}

fn tool_result(metadata: Value, image: Option<Vec<u8>>, failed: bool) -> CallToolResult {
    let mut content = Vec::new();
    if let Some(image) = image {
        content.push(ContentBlock::image(
            base64::engine::general_purpose::STANDARD.encode(image),
            "image/png",
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

fn observation_content(observation: Observation) -> (Value, Vec<u8>) {
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
    (metadata, observation.image.png)
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
    metadata["action_error"] = json!(report.error);
    metadata["observation_error"] = json!(report.observation_error);
    metadata["do_not_repeat_action"] = json!(true);
    let failed = report.error.is_some()
        || report.observation_error.is_some()
        || metadata["delivery"] == "unknown";
    let outcome = if failed {
        if report
            .error
            .into_iter()
            .chain(report.observation_error)
            .any(|error| error_outcome(error) == Outcome::Timeout)
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

fn action_rejected(message: &'static str) -> CallToolResult {
    tool_result(
        json!({"delivery":"not_sent", "action_error":message, "do_not_repeat_action":false}),
        None,
        true,
    )
}

fn message_result(value: Result<&'static str, &'static str>) -> (CallToolResult, Outcome) {
    match value {
        Ok(message) => (
            CallToolResult::success(vec![ContentBlock::text(message)]),
            Outcome::Success,
        ),
        Err(message) => (
            CallToolResult::error(vec![ContentBlock::text(message)]),
            error_outcome(message),
        ),
    }
}

#[tool_router(server_handler)]
impl TvServer {
    #[tool(description = "Connect to the configured TV without waking it.")]
    async fn tv_connect(&self) -> CallToolResult {
        let measurement = self.metrics.begin(Operation::Connect);
        let (result, outcome) = message_result(self.session.connect().await);
        measurement.finish(outcome);
        result
    }

    #[tool(
        description = "Capture a fresh TV image and observation ID, optionally waiting for pixel changes. A change or delay does not prove the UI settled."
    )]
    async fn tv_observe(&self, Parameters(params): Parameters<ObserveParams>) -> CallToolResult {
        let measurement = self.metrics.begin(Operation::Observe);
        let options = match observation_options(params.wait_ms, params.change_timeout_ms, false) {
            Ok(options) => options,
            Err(error) => {
                measurement.finish(Outcome::Failure);
                return tool_result(json!({"observation_error":error}), None, true);
            }
        };
        match self.session.observe(options).await {
            Ok(observation) => {
                let (metadata, image) = observation_content(observation);
                measurement.finish(Outcome::Success);
                tool_result(metadata, Some(image), false)
            }
            Err(error) => {
                measurement.finish(error_outcome(error));
                tool_result(json!({"observation_error":error}), None, true)
            }
        }
    }

    #[tool(
        description = "Send one to five paced button presses using the latest observation ID. Returns a new screenshot by default. Never repeat an action merely because its screenshot failed."
    )]
    async fn tv_press(&self, Parameters(params): Parameters<PressParams>) -> CallToolResult {
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
        let response = self
            .session
            .act(
                Action::Press {
                    button: button.into(),
                    count: params.count.unwrap_or(1),
                },
                &params.observation_id,
                params.observe.unwrap_or(true).then_some(options),
            )
            .await;
        let (result, outcome) = match response {
            Ok(report) => action_result(report),
            Err(error) => (action_rejected(error), error_outcome(error)),
        };
        measurement.finish(outcome);
        result
    }

    #[tool(
        description = "Launch an app using the latest observation ID; returns a new screenshot by default. Local send completion and visual confirmation are separate results."
    )]
    async fn tv_launch(&self, Parameters(params): Parameters<LaunchParams>) -> CallToolResult {
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
        let response = self
            .session
            .act(
                Action::Launch {
                    app_id: params.app_id,
                },
                &params.observation_id,
                params.observe.unwrap_or(true).then_some(options),
            )
            .await;
        let (result, outcome) = match response {
            Ok(report) => action_result(report),
            Err(error) => (action_rejected(error), error_outcome(error)),
        };
        measurement.finish(outcome);
        result
    }

    #[tool(description = "Close the TV session, reporting any unconfirmed cleanup.")]
    async fn tv_disconnect(&self) -> CallToolResult {
        let measurement = self.metrics.begin(Operation::Disconnect);
        let (result, outcome) = message_result(self.session.disconnect().await);
        measurement.finish(outcome);
        result
    }

    #[tool(
        description = "Read local aggregate timings and outcomes without contacting the TV. No screen, device, or request data is collected."
    )]
    fn tv_metrics(&self) -> CallToolResult {
        CallToolResult::structured(json!(self.metrics.snapshot()))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match setup::run_cli().await {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
    let metrics = Metrics::new(
        std::env::var_os("WEBOS_MCP_METRICS").as_deref() != Some(std::ffi::OsStr::new("0")),
    );
    let session = session::SessionOwner::new(metrics.clone());
    let server = TvServer {
        metrics,
        session: session.clone(),
    }
    .serve(stdio())
    .await?;
    let keepalive = session.start_keepalive();
    let result = server.waiting().await;
    keepalive.abort();
    let _ = keepalive.await;
    if session.disconnect().await.is_err() {
        eprintln!("TV session cleanup could not be confirmed.");
    }
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use session::Delivery;

    #[test]
    fn sent_action_with_failed_capture_preserves_delivery_and_returns_no_image() {
        let (result, outcome) = action_result(ActionReport {
            delivery: Delivery::Sent,
            error: None,
            observation: None,
            observation_error: Some("capture_timeout"),
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
        assert_eq!(metadata["observation_error"], "capture_timeout");
        assert_eq!(metadata["do_not_repeat_action"], true);
        assert!(metadata.get("observation_id").is_none());
    }

    #[test]
    fn uncertain_action_cannot_be_rendered_as_a_safe_rejection() {
        let (result, outcome) = action_result(ActionReport {
            delivery: Delivery::Unknown,
            error: Some("Button delivery is uncertain"),
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
                image: webos_mcp::capture::CapturedImage {
                    png: vec![1, 2, 3],
                    all_black: false,
                    fingerprint: [0; 32],
                    width: 1,
                    height: 1,
                    recovered: true,
                },
                id: "synthetic-session-2".into(),
                visual_change: session::VisualChange::Changed,
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
        assert_eq!(image.mime_type, "image/png");
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
