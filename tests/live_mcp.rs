use std::{fs::OpenOptions, io::Write, path::Path, time::Instant};

use base64::Engine;
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
};
use serde_json::json;
use tokio::process::Command;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

fn require_success(result: &CallToolResult, stage: &'static str) -> TestResult<()> {
    if result.is_error == Some(true) {
        return Err(format!("{stage} failed; no TV action will be retried").into());
    }
    Ok(())
}

fn save_observation(result: &CallToolResult, path: &Path) -> TestResult<String> {
    require_success(result, "observation")?;
    let metadata = result
        .structured_content
        .as_ref()
        .ok_or("observation has no structured metadata")?;
    if metadata["delivery_unresolved"] != false {
        return Err(
            "earlier action delivery is unresolved; live test will not send buttons".into(),
        );
    }
    let id = metadata["observation_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or("observation ID missing")?
        .to_owned();
    let image = result
        .content
        .iter()
        .find_map(|content| content.as_image())
        .ok_or("observation image missing")?;
    if image.mime_type != "image/png" {
        return Err("unexpected observation MIME type".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(&image.data)?;
    let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
    let mut reader = decoder.read_info()?;
    let mut pixels = vec![0; reader.output_buffer_size().ok_or("PNG size unavailable")?];
    let frame = reader.next_frame(&mut pixels)?;
    reader.finish()?;
    if metadata["width"] != frame.width || metadata["height"] != frame.height {
        return Err("observation dimensions do not match the returned PNG".into());
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&bytes)?;
    if metadata["all_black"] == true {
        return Err(
            "all-black capture saved; live navigation stopped without another action".into(),
        );
    }
    Ok(id)
}

fn require_sent(result: &CallToolResult) -> TestResult<()> {
    let metadata = result
        .structured_content
        .as_ref()
        .ok_or("action result has no structured delivery status")?;
    if metadata["delivery"] != "sent" || metadata["do_not_repeat_action"] != true {
        return Err("action send was not confirmed locally; test stops without replay".into());
    }
    if result.is_error == Some(true) {
        return Err("action was sent but its observation failed; test stops without replay".into());
    }
    Ok(())
}

#[tokio::test]
#[ignore = "controls the explicitly configured TV; requires a private evidence directory outside the repository"]
async fn observe_press_observe_through_mcp() -> TestResult<()> {
    let evidence_root = std::fs::canonicalize(std::env::var("WEBOS_MCP_EVIDENCE_DIR")?)?;
    let repository = std::fs::canonicalize(env!("CARGO_MANIFEST_DIR"))?;
    if !evidence_root.is_dir() || evidence_root.starts_with(repository) {
        return Err(
            "WEBOS_MCP_EVIDENCE_DIR must be an existing directory outside the repository".into(),
        );
    }
    let evidence = tempfile::Builder::new()
        .prefix("webos-mcp-live-")
        .tempdir_in(evidence_root)?
        .keep();
    eprintln!("Private evidence: {}", evidence.display());
    let client = ()
        .serve(TokioChildProcess::new(Command::new(env!(
            "CARGO_BIN_EXE_webos-mcp"
        )))?)
        .await?;
    let started = Instant::now();
    let journey: TestResult<()> = async {
        let connected = client.call_tool(CallToolRequestParams::new("tv_connect")).await?;
        require_success(&connected, "connect")?;
        let again = client.call_tool(CallToolRequestParams::new("tv_connect")).await?;
        require_success(&again, "repeated connect")?;
        if again.content.first().and_then(|content| content.as_text()).map(|content| content.text.as_str()) != Some("TV is already connected") {
            return Err("repeated connect did not reuse the session".into());
        }
        let before = client.call_tool(CallToolRequestParams::new("tv_observe")).await?;
        let before_id = save_observation(&before, &evidence.join("before.png"))?;
        let home = client.call_tool(CallToolRequestParams::new("tv_press").with_arguments(
            json!({"button":"HOME", "observation_id":before_id}).as_object().unwrap().clone()
        )).await?;
        require_sent(&home)?;
        let home_id = save_observation(&home, &evidence.join("after-home.png"))?;
        if home_id == before_id {
            return Err("combined action returned the previous observation ID".into());
        }
        // This intentionally stale request must be rejected before any button is sent.
        let stale = client.call_tool(CallToolRequestParams::new("tv_press").with_arguments(
            json!({"button":"RIGHT", "observation_id":before_id}).as_object().unwrap().clone()
        )).await?;
        let stale_metadata = stale.structured_content.as_ref().ok_or("stale rejection metadata missing")?;
        if stale.is_error != Some(true) || stale_metadata["delivery"] != "not_sent" || stale_metadata["action_error"] != "Observation ID is stale; observe before acting" {
            return Err("stale observation guard failed; no further action will be attempted".into());
        }
        let left = client.call_tool(CallToolRequestParams::new("tv_press").with_arguments(
            json!({"button":"LEFT", "observation_id":home_id}).as_object().unwrap().clone()
        )).await?;
        require_sent(&left)?;
        let final_id = save_observation(&left, &evidence.join("after-left.png"))?;
        if final_id == home_id {
            return Err("second combined action returned the previous observation ID".into());
        }
        let metrics = client.call_tool(CallToolRequestParams::new("tv_metrics")).await?;
        require_success(&metrics, "metrics")?;
        let snapshot = metrics.structured_content.as_ref().ok_or("metrics metadata missing")?;
        if snapshot["collection"] == "enabled" {
            if snapshot["operations"]["tv_press"]["count"] != 3 || snapshot["operations"]["tv_press"]["outcomes"]["success"] != 2 || snapshot["operations"]["tv_press"]["outcomes"]["failure"] != 1 {
                return Err("action metrics do not match this journey".into());
            }
            let samples = |result: &CallToolResult| -> TestResult<u64> {
                result.structured_content.as_ref()
                    .and_then(|metadata| metadata["samples"].as_u64())
                    .filter(|samples| *samples > 0)
                    .ok_or_else(|| "observation sample count missing".into())
            };
            for (operation, count) in [
                ("tv_observe", samples(&before)?),
                ("tv_press", samples(&home)? + samples(&left)?),
            ] {
                for stage in ["capture_request", "image_transfer", "image_validation"] {
                    if snapshot["stages"][operation][stage]["outcomes"]["success"] != count {
                        return Err("capture phase metrics do not match returned sample counts".into());
                    }
                }
            }
        } else if snapshot["collection"] != "disabled" {
            return Err("metrics collection status missing".into());
        }
        eprintln!("Protocol journey completed in {:.2} seconds; inspect the private images to verify visible behavior.", started.elapsed().as_secs_f64());
        Ok(())
    }.await;
    let cleanup: TestResult<()> = match client
        .call_tool(CallToolRequestParams::new("tv_disconnect"))
        .await
    {
        Ok(result) => require_success(&result, "disconnect"),
        Err(error) => Err(error.into()),
    };
    let shutdown = client.cancel().await;
    if cleanup.is_err() {
        eprintln!(
            "TV cleanup is unconfirmed; no automatic reconnect or action replay was attempted."
        );
    }
    journey?;
    cleanup?;
    shutdown?;
    Ok(())
}
