use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use tokio::process::Command;

/// One cold connect: reports per-step connect timings, sends no TV action.
#[tokio::test]
#[ignore = "connects to the configured TV, reports connect-stage timings, and disconnects"]
async fn report_connect_stage_timings() -> Result<(), Box<dyn std::error::Error>> {
    let client = ()
        .serve(TokioChildProcess::new(Command::new(env!(
            "CARGO_BIN_EXE_webos-mcp"
        )))?)
        .await?;
    let connected = client
        .call_tool(CallToolRequestParams::new("tv_connect"))
        .await?;
    let metrics = client
        .call_tool(CallToolRequestParams::new("tv_metrics"))
        .await?;
    let snapshot: Value = serde_json::from_str(
        &metrics.content[0]
            .as_text()
            .ok_or("metrics were not text")?
            .text,
    )?;
    let stages = &snapshot["stages"]["tv_connect"];
    let pick = |stage: &str| {
        let s = &stages[stage];
        json!({"max_ms": s["max_ms"], "outcomes": s["outcomes"], "count": s["count"]})
    };
    let disconnected = client
        .call_tool(CallToolRequestParams::new("tv_disconnect"))
        .await?;
    eprintln!(
        "CONNECT_STAGES {}",
        json!({
            "connected": connected.is_error != Some(true),
            "disconnect_confirmed": disconnected.is_error != Some(true),
            "connect_ms": snapshot["operations"]["tv_connect"]["max_ms"],
            "tcp_connect": pick("tcp_connect"),
            "tls_handshake": pick("tls_handshake"),
            "websocket_handshake": pick("websocket_handshake"),
            "registration": pick("registration"),
        })
    );
    client.cancel().await?;
    assert_ne!(
        disconnected.is_error,
        Some(true),
        "TV cleanup was unconfirmed"
    );
    Ok(())
}
