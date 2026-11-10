//! The CLI and the MCP server must return the same result for the same calls.
//! These journeys need no TV: a fresh server is disconnected, and none of the
//! calls reads credentials.
use std::process::Stdio;

use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt, process::Command};

mod support;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

/// The same call in both spellings: a CLI line, and an MCP tool with arguments.
fn journey() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("tv_metrics", "tv_metrics", json!({})),
        ("tv_observe", "tv_observe", json!({})),
        (
            "tv_observe --wait-ms 5001",
            "tv_observe",
            json!({"wait_ms": 5001}),
        ),
        (
            "tv_observe --change-timeout-ms 3001",
            "tv_observe",
            json!({"change_timeout_ms": 3001}),
        ),
        (
            "tv_press --button RIGHT --observation-id synthetic-1",
            "tv_press",
            json!({"button": "RIGHT", "observation_id": "synthetic-1"}),
        ),
        (
            "tv_press --button UP --observation-id synthetic-1 --count 5 --observe false",
            "tv_press",
            json!({"button": "UP", "observation_id": "synthetic-1", "count": 5, "observe": false}),
        ),
        (
            "tv_press --button UP --observation-id synthetic-1 --count 6",
            "tv_press",
            json!({"button": "UP", "observation_id": "synthetic-1", "count": 6}),
        ),
        (
            "tv_press --button UP --observation-id ' '",
            "tv_press",
            json!({"button": "UP", "observation_id": " "}),
        ),
        (
            "tv_press --button HOME --observation-id latest",
            "tv_press",
            json!({"button": "HOME", "observation_id": "latest"}),
        ),
        (
            "tv_launch --app-id example.app --observation-id synthetic-1 --wait-ms 9999",
            "tv_launch",
            json!({"app_id": "example.app", "observation_id": "synthetic-1", "wait_ms": 9999}),
        ),
        (
            "tv_launch --app-id ' ' --observation-id synthetic-1",
            "tv_launch",
            json!({"app_id": " ", "observation_id": "synthetic-1"}),
        ),
        (
            "tv_launch --app-id example.app --observation-id synthetic-1",
            "tv_launch",
            json!({"app_id": "example.app", "observation_id": "synthetic-1"}),
        ),
        ("tv_disconnect", "tv_disconnect", json!({})),
        ("tv_metrics", "tv_metrics", json!({})),
    ]
}

fn binary() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_webos-mcp"));
    command.env("WEBOS_MCP_METRICS", "1");
    command
}

async fn through_mcp() -> TestResult<Vec<Value>> {
    let client = ().serve(TokioChildProcess::new(binary())?).await?;
    let mut results = Vec::new();
    for (_, tool, arguments) in journey() {
        let request =
            CallToolRequestParams::new(tool).with_arguments(arguments.as_object().unwrap().clone());
        let result = serde_json::to_value(client.call_tool(request).await?)?;
        results.push(support::comparable(tool, result));
    }
    client.cancel().await?;
    Ok(results)
}

/// Run the CLI and return its exit success and one parsed result per line.
async fn through_cli(arguments: &[&str], stdin: Option<String>) -> TestResult<(bool, Vec<Value>)> {
    let mut child = binary()
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut input = child.stdin.take().ok_or("no stdin")?;
    if let Some(lines) = stdin {
        input.write_all(lines.as_bytes()).await?;
    }
    drop(input);
    let output = child.wait_with_output().await?;
    let mut results = Vec::new();
    for line in String::from_utf8(output.stdout)?.lines() {
        let mut printed: Value = serde_json::from_str(line)?;
        let tool = printed["tool"].as_str().ok_or("no tool name")?.to_owned();
        results.push(support::comparable(&tool, printed["result"].take()));
    }
    Ok((output.status.success(), results))
}

#[tokio::test]
async fn session_results_equal_mcp_results_for_the_same_calls() -> TestResult<()> {
    let expected = through_mcp().await?;
    let lines: Vec<&str> = journey().iter().map(|(line, _, _)| *line).collect();

    let script = format!(
        "# comments and blank lines are skipped\n\n{}\n",
        lines.join("\n")
    );
    let (succeeded, from_stdin) = through_cli(&["session", "--keep-going"], Some(script)).await?;
    assert!(
        !succeeded,
        "a journey with rejected calls must exit nonzero"
    );
    assert_eq!(from_stdin.len(), expected.len());
    for ((line, actual), wanted) in lines.iter().zip(&from_stdin).zip(&expected) {
        assert_eq!(actual, wanted, "{line}");
    }

    let mut arguments = vec!["session", "--keep-going"];
    for line in &lines {
        arguments.extend(["--call", line]);
    }
    let (_, from_options) = through_cli(&arguments, None).await?;
    assert_eq!(from_options, expected);

    // The journey is not vacuous: it reached handlers, validation, and metrics.
    assert_eq!(
        expected[1]["structuredContent"]["observation_error"],
        "TV is disconnected"
    );
    assert_eq!(expected[4]["structuredContent"]["delivery"], "not_sent");
    assert_eq!(
        expected[6]["structuredContent"]["action_error"],
        "count must be between 1 and 5."
    );
    assert_eq!(expected[12]["isError"], false);
    let last = &expected[13]["structuredContent"]["operations"];
    assert_eq!(last["tv_press"]["count"], 5);
    assert_eq!(last["tv_observe"]["outcomes"]["failure"], 3);
    Ok(())
}

#[tokio::test]
async fn one_shot_call_equals_the_first_mcp_result_and_sets_the_exit_status() -> TestResult<()> {
    let client = ().serve(TokioChildProcess::new(binary())?).await?;
    let observe = serde_json::to_value(
        client
            .call_tool(CallToolRequestParams::new("tv_observe"))
            .await?,
    )?;
    let press = serde_json::to_value(
        client
            .call_tool(
                CallToolRequestParams::new("tv_press").with_arguments(
                    json!({"button": "LEFT", "observation_id": "x"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await?,
    )?;
    client.cancel().await?;

    let (succeeded, results) = through_cli(&["call", "tv_observe"], None).await?;
    assert!(!succeeded);
    assert_eq!(results, [support::comparable("tv_observe", observe)]);
    let (succeeded, results) = through_cli(
        &[
            "call",
            "tv_press",
            "--button",
            "LEFT",
            "--observation-id",
            "x",
        ],
        None,
    )
    .await?;
    assert!(!succeeded);
    assert_eq!(results, [support::comparable("tv_press", press)]);
    let (succeeded, results) = through_cli(&["call", "tv_disconnect"], None).await?;
    assert!(succeeded);
    assert_eq!(
        results[0]["content"][0]["text"],
        "TV is already disconnected"
    );
    Ok(())
}

#[tokio::test]
async fn a_session_stops_at_the_first_failure_unless_told_to_continue() -> TestResult<()> {
    let script = "tv_metrics\ntv_observe\ntv_press --button RIGHT --observation-id x\n";
    let (succeeded, results) = through_cli(&["session"], Some(script.into())).await?;
    assert!(!succeeded);
    assert_eq!(
        results.len(),
        2,
        "nothing runs after the failed observation"
    );

    let unparsable = "tv_metrics\ntv_press --button SIDEWAYS --observation-id x\ntv_metrics\n";
    let (succeeded, results) = through_cli(&["session"], Some(unparsable.into())).await?;
    assert!(!succeeded);
    assert_eq!(results.len(), 1, "a malformed call reaches no handler");

    let (succeeded, results) =
        through_cli(&["session"], Some("tv_metrics\ntv_disconnect\n".into())).await?;
    assert!(succeeded);
    assert_eq!(results.len(), 2);
    Ok(())
}

#[tokio::test]
async fn management_commands_and_usage_errors_keep_working() -> TestResult<()> {
    let help = binary().arg("--help").output().await?;
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout)?;
    for command in [
        "setup",
        "use-local-file",
        "acknowledge-delivery",
        "remove",
        "call",
        "session",
    ] {
        assert!(text.contains(command), "{command}");
    }
    let unknown = binary().arg("bogus").output().await?;
    assert!(!unknown.status.success());
    assert!(
        unknown.stdout.is_empty(),
        "usage errors must not reach stdout"
    );
    Ok(())
}

/// Send a signal to a child process the way a terminal or a supervisor would.
#[cfg(unix)]
async fn signal(child: &tokio::process::Child, name: &str) -> TestResult<()> {
    let id = child.id().ok_or("child already exited")?.to_string();
    let sent = Command::new("kill").args([name, &id]).status().await?;
    assert!(sent.success());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ctrl_c_ends_a_session_through_the_normal_shutdown() -> TestResult<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut child = binary()
        .arg("session")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
    stdin.write_all(b"tv_metrics\n").await?;
    let mut first = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stdout.read_line(&mut first),
    )
    .await??;
    assert!(first.contains("tv_metrics"));
    // Stdin stays open, so only the signal can end the session.
    signal(&child, "-INT").await?;
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(5), child.wait_with_output()).await??;
    assert_eq!(
        output.status.code(),
        Some(130),
        "handled, not killed by the signal"
    );
    assert!(String::from_utf8(output.stderr)?.contains("Interrupted"));
    drop(stdin);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn sigterm_stops_the_mcp_server_through_the_normal_shutdown() -> TestResult<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut child = binary()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
    stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test-client\",\"version\":\"1\"}}}\n").await?;
    let mut response = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stdout.read_line(&mut response),
    )
    .await??;
    assert!(response.contains("\"result\""));
    stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await?;
    // Stdin stays open, so only the signal can stop the server.
    signal(&child, "-TERM").await?;
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(5), child.wait_with_output()).await??;
    assert_eq!(
        output.status.code(),
        Some(0),
        "handled, not killed by the signal"
    );
    drop(stdin);
    Ok(())
}
