use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use tokio::process::Command;

#[tokio::test]
#[ignore = "connects to the configured TV and observes across an idle interval"]
async fn observe_after_idle_without_tv_actions() -> Result<(), Box<dyn std::error::Error>> {
    let client = ()
        .serve(TokioChildProcess::new(Command::new(env!(
            "CARGO_BIN_EXE_webos-mcp"
        )))?)
        .await?;
    let connected = client
        .call_tool(CallToolRequestParams::new("tv_connect"))
        .await?;
    assert_ne!(connected.is_error, Some(true), "TV connection failed");
    let first = client
        .call_tool(CallToolRequestParams::new("tv_observe"))
        .await?;
    assert_ne!(first.is_error, Some(true), "initial capture failed");
    assert!(
        first
            .content
            .iter()
            .any(|content| content.as_image().is_some())
    );
    eprintln!("Initial capture succeeded; waiting across the idle interval.");
    tokio::time::sleep(std::time::Duration::from_secs(50)).await;
    let second = client
        .call_tool(CallToolRequestParams::new("tv_observe"))
        .await?;
    assert_ne!(second.is_error, Some(true), "capture after idle failed");
    assert!(
        second
            .content
            .iter()
            .any(|content| content.as_image().is_some())
    );
    let healthy = client
        .call_tool(CallToolRequestParams::new("tv_connect"))
        .await?;
    assert_ne!(
        healthy.is_error,
        Some(true),
        "control connections were stale after idle"
    );
    let closed = client
        .call_tool(CallToolRequestParams::new("tv_disconnect"))
        .await?;
    assert_ne!(closed.is_error, Some(true), "TV cleanup was unconfirmed");
    client.cancel().await?;
    Ok(())
}
