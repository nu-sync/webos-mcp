use std::{collections::BTreeSet, process::Stdio, time::Duration};

use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

fn args(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().unwrap().clone()
}
fn text(result: &CallToolResult) -> &str {
    &result.content[0].as_text().expect("text result").text
}

#[tokio::test]
async fn mcp_client_exercises_validation_and_metrics_without_a_tv()
-> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_webos-mcp"));
    command.env("WEBOS_MCP_METRICS", "1");
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    let tools = client.list_all_tools().await?;
    let names: BTreeSet<_> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "tv_connect",
            "tv_disconnect",
            "tv_launch",
            "tv_metrics",
            "tv_observe",
            "tv_press"
        ])
    );
    let press = tools.iter().find(|tool| tool.name == "tv_press").unwrap();
    let observe = tools.iter().find(|tool| tool.name == "tv_observe").unwrap();
    let schema = serde_json::to_value(&press.input_schema)?;
    assert_eq!(schema["properties"]["count"]["minimum"], 1);
    assert_eq!(schema["properties"]["count"]["maximum"], 5);
    assert_eq!(schema["required"], json!(["button", "observation_id"]));
    let observe_schema = serde_json::to_value(&observe.input_schema)?;
    assert_eq!(observe_schema["properties"]["wait_ms"]["maximum"], 5000);
    assert_eq!(
        observe_schema["properties"]["change_timeout_ms"]["maximum"],
        3000
    );

    let result = client
        .call_tool(CallToolRequestParams::new("tv_observe"))
        .await?;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.as_ref().unwrap()["observation_error"],
        "TV is disconnected"
    );
    for (value, message) in [
        (json!({"wait_ms":5001}), "wait_ms"),
        (json!({"change_timeout_ms":3001}), "change_timeout_ms"),
    ] {
        let result = client
            .call_tool(CallToolRequestParams::new("tv_observe").with_arguments(args(value)))
            .await?;
        assert_eq!(result.is_error, Some(true));
        assert!(text(&result).contains(message));
    }
    for value in [
        json!({"button":"RIGHT", "observation_id":"synthetic-1"}),
        json!({"button":"UP", "observation_id":"synthetic-1", "count":5, "observe":false}),
    ] {
        let result = client
            .call_tool(CallToolRequestParams::new("tv_press").with_arguments(args(value)))
            .await?;
        assert_eq!(result.is_error, Some(true));
        let metadata = result.structured_content.unwrap();
        assert_eq!(metadata["delivery"], "not_sent");
        assert_eq!(metadata["action_error"], "TV is disconnected");
        assert_eq!(metadata["do_not_repeat_action"], false);
    }
    for (value, message) in [
        (
            json!({"button":"UP","observation_id":"synthetic-1","count":0}),
            "between 1 and 5",
        ),
        (
            json!({"button":"UP","observation_id":"synthetic-1","count":6}),
            "between 1 and 5",
        ),
        (
            json!({"button":"UP","observation_id":" "}),
            "observation_id",
        ),
        (
            json!({"button":"UP","observation_id":"synthetic-1","wait_ms":5001}),
            "wait_ms",
        ),
        (
            json!({"button":"UP","observation_id":"synthetic-1","change_timeout_ms":3001}),
            "change_timeout_ms",
        ),
    ] {
        let result = client
            .call_tool(CallToolRequestParams::new("tv_press").with_arguments(args(value)))
            .await?;
        assert_eq!(result.is_error, Some(true));
        assert!(text(&result).contains(message));
        assert_eq!(result.structured_content.unwrap()["delivery"], "not_sent");
    }
    for (value, expected) in [
        (
            json!({"app_id":"example.app","observation_id":"synthetic-1"}),
            "TV is disconnected",
        ),
        (
            json!({"app_id":" ","observation_id":"synthetic-1"}),
            "app_id must not be empty.",
        ),
        (
            json!({"app_id":"example.app","observation_id":""}),
            "observation_id must not be empty.",
        ),
    ] {
        let result = client
            .call_tool(CallToolRequestParams::new("tv_launch").with_arguments(args(value)))
            .await?;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content.unwrap()["action_error"], expected);
    }
    for _ in 0..2 {
        let result = client
            .call_tool(CallToolRequestParams::new("tv_disconnect"))
            .await?;
        assert_ne!(result.is_error, Some(true));
        assert_eq!(text(&result), "TV is already disconnected");
    }
    for _ in 0..2 {
        let result = client
            .call_tool(CallToolRequestParams::new("tv_metrics"))
            .await?;
        let metrics = result.structured_content.unwrap();
        assert_eq!(metrics["scope"], "current_server_process");
        assert_eq!(metrics["collection"], "enabled");
        assert_eq!(metrics["operations"]["tv_observe"]["count"], 3);
        assert_eq!(metrics["operations"]["tv_press"]["count"], 7);
        assert_eq!(metrics["operations"]["tv_press"]["outcomes"]["failure"], 7);
        assert_eq!(metrics["operations"]["tv_launch"]["count"], 3);
        assert_eq!(
            metrics["operations"]["tv_disconnect"]["outcomes"]["success"],
            2
        );
        assert_eq!(metrics["operations"]["tv_connect"]["count"], 0);
        assert_eq!(metrics["operations"]["tv_connect"]["mean_ms"], Value::Null);
        assert_eq!(metrics["operations"].as_object().unwrap().len(), 5);
    }
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn mcp_rejects_malformed_parameters_before_session_work()
-> Result<(), Box<dyn std::error::Error>> {
    let client = ()
        .serve(TokioChildProcess::new(Command::new(env!(
            "CARGO_BIN_EXE_webos-mcp"
        )))?)
        .await?;
    for (name, value) in [
        ("tv_press", json!({"button":"UP"})),
        (
            "tv_press",
            json!({"button":"INVALID","observation_id":"synthetic-1"}),
        ),
        (
            "tv_press",
            json!({"button":"UP","observation_id":"synthetic-1","extra":true}),
        ),
        ("tv_press", json!({"button":"UP","observation_id":1})),
        ("tv_launch", json!({"app_id":"example.app"})),
        ("tv_observe", json!({"wait_ms":-1})),
        ("tv_observe", json!({"extra":true})),
    ] {
        match client
            .call_tool(CallToolRequestParams::new(name).with_arguments(args(value)))
            .await
        {
            Err(_) => {}
            Ok(result) => {
                assert_eq!(result.is_error, Some(true));
                assert!(!text(&result).contains("TV is disconnected"));
                assert!(result.structured_content.is_none());
            }
        }
    }
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn disabled_metrics_report_zero_samples() -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_webos-mcp"));
    command.env("WEBOS_MCP_METRICS", "0");
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    let result = client
        .call_tool(CallToolRequestParams::new("tv_observe"))
        .await?;
    assert_eq!(result.is_error, Some(true));
    let result = client
        .call_tool(CallToolRequestParams::new("tv_metrics"))
        .await?;
    let metrics = result.structured_content.unwrap();
    assert_eq!(metrics["collection"], "disabled");
    assert_eq!(metrics["operations"]["tv_observe"]["count"], 0);
    assert_eq!(
        metrics["operations"]["tv_observe"]["outcomes"]["failure"],
        0
    );
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn server_exits_cleanly_on_stdin_eof() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_webos-mcp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test-client\",\"version\":\"1\"}}}\n").await?;
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(5), stdout.read_line(&mut response)).await??;
    let parsed: Value = serde_json::from_str(&response)?;
    assert_eq!(parsed["id"], 1);
    assert!(parsed.get("result").is_some());
    stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await?;
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await??;
    assert!(output.status.success());
    let mut trailing = String::new();
    stdout.read_line(&mut trailing).await?;
    assert!(trailing.is_empty());
    Ok(())
}
