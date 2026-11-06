use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use tokio::process::Command;

#[tokio::test]
#[ignore = "connects to the configured TV and verifies session cleanup"]
async fn disconnect_owned_tv_session() -> Result<(), Box<dyn std::error::Error>> {
    let client = ()
        .serve(TokioChildProcess::new(Command::new(env!(
            "CARGO_BIN_EXE_webos-mcp"
        )))?)
        .await?;
    let connected = client
        .call_tool(CallToolRequestParams::new("tv_connect"))
        .await?;
    assert_ne!(connected.is_error, Some(true), "TV connection failed");
    let disconnected = client
        .call_tool(CallToolRequestParams::new("tv_disconnect"))
        .await?;
    assert_ne!(
        disconnected.is_error,
        Some(true),
        "TV cleanup was unconfirmed"
    );
    client.cancel().await?;
    Ok(())
}
