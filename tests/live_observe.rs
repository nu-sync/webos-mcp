use std::{fs::OpenOptions, io::Write, path::Path, time::Instant};

use base64::Engine;
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use tokio::process::Command;

mod support;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

fn save_private(path: &Path, bytes: &[u8]) -> TestResult<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)?;
    Ok(())
}

/// One cold session: connect, capture once, read stage timings, disconnect.
/// Sends no button or launch.
async fn observe_once(binary: &str, image_path: &Path) -> TestResult<Value> {
    let client = ().serve(TokioChildProcess::new(Command::new(binary))?).await?;
    let trial: TestResult<Value> = async {
        let connected = client
            .call_tool(CallToolRequestParams::new("tv_connect"))
            .await?;
        if connected.is_error == Some(true) {
            return Err("connect failed".into());
        }
        let started = Instant::now();
        let observed = client
            .call_tool(CallToolRequestParams::new("tv_observe"))
            .await?;
        let observe_ms = started.elapsed().as_secs_f64() * 1000.0;
        if observed.is_error == Some(true) {
            return Err("observation failed".into());
        }
        let metadata = observed
            .structured_content
            .as_ref()
            .ok_or("observation metadata missing")?;
        let image = observed
            .content
            .iter()
            .find_map(|content| content.as_image())
            .ok_or("observation image missing")?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&image.data)?;
        let (width, height) = support::decoded_dimensions(&image.mime_type, &bytes)?;
        if metadata["width"] != width || metadata["height"] != height {
            return Err("image dimensions disagree with metadata".into());
        }
        save_private(
            &support::evidence_path(image_path, &image.mime_type),
            &bytes,
        )?;
        let metrics = client
            .call_tool(CallToolRequestParams::new("tv_metrics"))
            .await?;
        let snapshot = metrics
            .structured_content
            .as_ref()
            .ok_or("metrics metadata missing")?;
        let stages = &snapshot["stages"]["tv_observe"];
        Ok(json!({
            "mime_type": image.mime_type,
            "image_bytes": bytes.len(),
            "width": width,
            "height": height,
            "all_black": metadata["all_black"],
            "delivery_unresolved": metadata["delivery_unresolved"],
            "observe_ms": observe_ms,
            "capture_request_ms": stages["capture_request"]["max_ms"],
            "image_transfer_ms": stages["image_transfer"]["max_ms"],
            "image_validation_ms": stages["image_validation"]["max_ms"],
        }))
    }
    .await;
    let disconnected = client
        .call_tool(CallToolRequestParams::new("tv_disconnect"))
        .await;
    client.cancel().await?;
    let trial = trial?;
    if disconnected?.is_error == Some(true) {
        return Err("TV cleanup was unconfirmed; stopping without another connection".into());
    }
    Ok(trial)
}

/// Read-only capture trials through the compiled server. Set
/// `WEBOS_MCP_BASELINE_BIN` to another server binary to alternate it with this
/// build on the same screen; any failure stops the run.
#[tokio::test]
#[ignore = "connects to the configured TV and captures screenshots without sending any action; requires a private evidence directory outside the repository"]
async fn observe_without_tv_actions() -> TestResult<()> {
    let evidence_root = std::fs::canonicalize(std::env::var("WEBOS_MCP_EVIDENCE_DIR")?)?;
    let repository = std::fs::canonicalize(env!("CARGO_MANIFEST_DIR"))?;
    if !evidence_root.is_dir() || evidence_root.starts_with(repository) {
        return Err(
            "WEBOS_MCP_EVIDENCE_DIR must be an existing directory outside the repository".into(),
        );
    }
    let evidence = tempfile::Builder::new()
        .prefix("webos-mcp-observe-")
        .tempdir_in(evidence_root)?
        .keep();
    eprintln!("Private evidence: {}", evidence.display());
    let rounds: usize = match std::env::var("WEBOS_MCP_OBSERVE_ROUNDS") {
        Ok(value) => value.parse()?,
        Err(_) => 1,
    };
    if !(1..=10).contains(&rounds) {
        return Err("WEBOS_MCP_OBSERVE_ROUNDS must be between 1 and 10".into());
    }
    let mut candidates = vec![("candidate", env!("CARGO_BIN_EXE_webos-mcp").to_owned())];
    if let Ok(baseline) = std::env::var("WEBOS_MCP_BASELINE_BIN") {
        candidates.push(("baseline", baseline));
    }
    for round in 0..rounds {
        // Alternate the order so neither binary always runs on a warmer TV.
        let mut order = candidates.clone();
        if round % 2 == 1 {
            order.reverse();
        }
        for (label, binary) in order {
            let image = evidence.join(format!("round-{round}-{label}.png"));
            let mut result = observe_once(&binary, &image).await?;
            result["round"] = json!(round);
            result["binary"] = json!(label);
            eprintln!("OBSERVE_TRIAL {result}");
        }
    }
    Ok(())
}
