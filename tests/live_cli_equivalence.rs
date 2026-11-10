//! Read-only comparison of the CLI and MCP front ends against the configured TV.
use std::process::Stdio;

use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use tokio::process::Command;

mod support;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

/// Calls that connect, look, and disconnect. None sends a button or launch.
const JOURNEY: [(&str, &str, &str); 6] = [
    ("tv_connect", "tv_connect", "{}"),
    ("tv_connect", "tv_connect", "{}"),
    ("tv_observe", "tv_observe", "{}"),
    (
        "tv_observe --wait-ms 100",
        "tv_observe",
        r#"{"wait_ms":100}"#,
    ),
    ("tv_metrics", "tv_metrics", "{}"),
    ("tv_disconnect", "tv_disconnect", "{}"),
];

use support::comparable_session as comparable;

#[tokio::test]
#[ignore = "connects to the configured TV twice and captures screenshots without sending any action; requires a private evidence directory outside the repository"]
async fn cli_session_matches_mcp_on_a_read_only_journey() -> TestResult<()> {
    let evidence_root = std::fs::canonicalize(std::env::var("WEBOS_MCP_EVIDENCE_DIR")?)?;
    let repository = std::fs::canonicalize(env!("CARGO_MANIFEST_DIR"))?;
    if !evidence_root.is_dir() || evidence_root.starts_with(repository) {
        return Err(
            "WEBOS_MCP_EVIDENCE_DIR must be an existing directory outside the repository".into(),
        );
    }
    let evidence = tempfile::Builder::new()
        .prefix("webos-mcp-cli-")
        .tempdir_in(evidence_root)?
        .keep();
    eprintln!("Private evidence: {}", evidence.display());

    let client = ()
        .serve(TokioChildProcess::new(Command::new(env!(
            "CARGO_BIN_EXE_webos-mcp"
        )))?)
        .await?;
    let mut through_mcp = Vec::new();
    let mut mcp_dimensions = Vec::new();
    for (_, tool, arguments) in JOURNEY {
        let arguments: Value = serde_json::from_str(arguments)?;
        let request =
            CallToolRequestParams::new(tool).with_arguments(arguments.as_object().unwrap().clone());
        let result = client.call_tool(request).await?;
        if let Some(image) = result.content.iter().find_map(|block| block.as_image()) {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD.decode(&image.data)?;
            mcp_dimensions.push(support::decoded_dimensions(&image.mime_type, &bytes)?);
        }
        through_mcp.push(comparable(tool, serde_json::to_value(result)?));
    }
    client.cancel().await?;
    for ((line, _, _), result) in JOURNEY.iter().zip(&through_mcp) {
        if result["isError"] == true {
            // Messages are fixed strings and carry no device details.
            let reason = result
                .pointer("/structuredContent/observation_error")
                .or_else(|| result.pointer("/content/0/text"));
            return Err(format!(
                "MCP `{line}` failed ({reason:?}); the CLI journey was not started"
            )
            .into());
        }
    }

    let mut command = Command::new(env!("CARGO_BIN_EXE_webos-mcp"));
    command.arg("session").arg("--image-dir").arg(&evidence);
    for (line, _, _) in JOURNEY {
        command.arg("--call").arg(line);
    }
    let output = command.stdin(Stdio::null()).output().await?;
    assert!(output.status.success(), "CLI session exited with a failure");
    let mut through_cli = Vec::new();
    let mut cli_dimensions = Vec::new();
    for line in String::from_utf8(output.stdout)?.lines() {
        let mut printed: Value = serde_json::from_str(line)?;
        let tool = printed["tool"].as_str().ok_or("no tool name")?.to_owned();
        let result = printed["result"].take();
        for block in result["content"].as_array().into_iter().flatten() {
            if block["type"] == "image" {
                let path = block["path"].as_str().ok_or("image was not saved")?;
                let bytes = std::fs::read(path)?;
                assert_eq!(block["bytes"], json!(bytes.len()));
                let mime_type = block["mimeType"].as_str().ok_or("no MIME type")?;
                cli_dimensions.push(support::decoded_dimensions(mime_type, &bytes)?);
            }
        }
        through_cli.push(comparable(&tool, result));
    }

    assert_eq!(through_cli.len(), JOURNEY.len());
    for (((line, _, _), cli), mcp) in JOURNEY.iter().zip(&through_cli).zip(&through_mcp) {
        assert_eq!(cli, mcp, "{line}");
    }
    assert_eq!(cli_dimensions.len(), 2);
    assert_eq!(cli_dimensions, mcp_dimensions);
    eprintln!(
        "CLI_EQUIVALENCE {}",
        json!({
            "calls": JOURNEY.len(),
            "images": cli_dimensions.len(),
            "dimensions": cli_dimensions,
            "connect": through_cli[0]["content"][0]["text"],
            "repeat_connect": through_cli[1]["content"][0]["text"],
            "disconnect": through_cli[5]["content"][0]["text"],
            "observe_metadata": through_cli[2]["structuredContent"],
        })
    );
    Ok(())
}
