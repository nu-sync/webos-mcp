use std::{fs::OpenOptions, io::Write, path::Path, time::Instant};

use base64::Engine;
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    service::Peer,
    transport::TokioChildProcess,
};
use serde_json::{Value, json};
use tokio::process::Command;
use webos_mcp::metrics::{Metrics, Operation, Outcome};

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

fn metadata(result: &CallToolResult) -> TestResult<&Value> {
    result
        .structured_content
        .as_ref()
        .ok_or_else(|| "missing structured result".into())
}

fn require_success(result: &CallToolResult) -> TestResult<()> {
    if result.is_error == Some(true) {
        return Err("tool reported failure; benchmark stops without replay".into());
    }
    Ok(())
}

fn require_sent(result: &CallToolResult) -> TestResult<()> {
    require_success(result)?;
    let data = metadata(result)?;
    if data["delivery"] != "sent" || data["do_not_repeat_action"] != true {
        return Err("action delivery is unconfirmed; benchmark stops without replay".into());
    }
    Ok(())
}

fn observation(result: &CallToolResult, previous: &str, path: &Path) -> TestResult<(String, bool)> {
    require_success(result)?;
    let data = metadata(result)?;
    if data["delivery_unresolved"] != false {
        return Err("unresolved delivery prevents further actions".into());
    }
    let id = data["observation_id"]
        .as_str()
        .filter(|id| !id.is_empty() && *id != previous)
        .ok_or("observation ID is missing or stale")?;
    let image = result
        .content
        .iter()
        .find_map(|content| content.as_image())
        .ok_or("fresh screenshot missing")?;
    if image.mime_type != "image/png" {
        return Err("unexpected screenshot type".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(&image.data)?;
    let mut reader = png::Decoder::new(std::io::Cursor::new(&bytes)).read_info()?;
    let size = reader.output_buffer_size().ok_or("PNG size unavailable")?;
    if size > 64 * 1024 * 1024 {
        return Err("PNG exceeds test decode limit".into());
    }
    let mut pixels = vec![0; size];
    let frame = reader.next_frame(&mut pixels)?;
    reader.finish()?;
    if data["width"] != frame.width || data["height"] != frame.height {
        return Err("screenshot dimensions disagree with metadata".into());
    }
    save_private(path, &bytes)?;
    if data["all_black"] != false || data["visual_change"] == "black" {
        return Err("black screenshot saved; benchmark stops without another action".into());
    }
    Ok((id.to_owned(), data["visual_change"] == "changed"))
}

async fn timed_call(
    client: &Peer<RoleClient>,
    name: &'static str,
    arguments: Value,
    calls: &mut Vec<Value>,
) -> TestResult<CallToolResult> {
    let request = CallToolRequestParams::new(name).with_arguments(
        arguments
            .as_object()
            .ok_or("invalid test arguments")?
            .clone(),
    );
    let started = Instant::now();
    let result = client.call_tool(request).await;
    calls.push(json!({
        "tool": name,
        "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0,
        "rpc_completed": result.is_ok(),
        "tool_error": result.as_ref().ok().map(|result| result.is_error == Some(true)),
    }));
    result.map_err(|_| "MCP request failed; benchmark stops without replay".into())
}

fn summary(values: &[f64]) -> Value {
    if values.is_empty() {
        return json!({"count": 0, "min_ms": null, "median_ms": null, "max_ms": null});
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    };
    json!({"count": sorted.len(), "min_ms": sorted[0], "median_ms": median, "max_ms": sorted[sorted.len()-1]})
}

#[tokio::test]
#[ignore = "moves focus on the configured TV; requires an external private evidence directory and a static menu"]
async fn compare_split_and_combined_control_pairs() -> TestResult<()> {
    let root = std::fs::canonicalize(std::env::var("WEBOS_MCP_EVIDENCE_DIR")?)?;
    let repository = std::fs::canonicalize(env!("CARGO_MANIFEST_DIR"))?;
    if !root.is_dir() || root.starts_with(repository) {
        return Err("evidence directory must already exist outside the repository".into());
    }
    let evidence = tempfile::Builder::new()
        .prefix("webos-mcp-performance-")
        .tempdir_in(root)?
        .keep();
    eprintln!("Private benchmark evidence: {}", evidence.display());
    let client = ()
        .serve(TokioChildProcess::new(Command::new(env!(
            "CARGO_BIN_EXE_webos-mcp"
        )))?)
        .await?;
    let mut trials = Vec::new();
    let mut auxiliary_calls = Vec::new();
    let mut split = Vec::new();
    let mut combined = Vec::new();
    let journey: TestResult<()> =
        async {
            require_success(
                &timed_call(&client, "tv_connect", json!({}), &mut auxiliary_calls).await?,
            )?;
            let initial =
                timed_call(&client, "tv_observe", json!({}), &mut auxiliary_calls).await?;
            let (mut id, _) = observation(&initial, "", &evidence.join("initial.png"))?;
            for round in 0..5 {
                let order = if round % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                };
                for is_combined in order {
                    let candidate = if is_combined { "combined" } else { "split" };
                    let mut calls = Vec::new();
                    let mut changes = 0;
                    let started = Instant::now();
                    let pair: TestResult<()> =
                        async {
                            for button in ["RIGHT", "LEFT"] {
                                let action = timed_call(&client, "tv_press", json!({
                            "button": button, "observation_id": id, "observe": is_combined
                        }), &mut calls).await?;
                                require_sent(&action)?;
                                let frame = if is_combined {
                                    action
                                } else {
                                    timed_call(
                                        &client,
                                        "tv_observe",
                                        json!({
                                            "wait_ms": 250, "change_timeout_ms": 1000
                                        }),
                                        &mut calls,
                                    )
                                    .await?
                                };
                                let path = evidence
                                    .join(format!("round-{round}-{candidate}-{button}.png"));
                                let (next, changed) = observation(&frame, &id, &path)?;
                                id = next;
                                changes += u64::from(changed);
                            }
                            Ok(())
                        }
                        .await;
                    let rpc_ms: f64 = calls
                        .iter()
                        .filter_map(|call| call["elapsed_ms"].as_f64())
                        .sum();
                    trials.push(json!({
                    "round": round, "candidate": candidate, "completed": pair.is_ok(),
                    "pair_rpc_ms": rpc_ms, "pair_wall_ms": started.elapsed().as_secs_f64()*1000.0,
                    "pixel_changed_observations": changes, "calls": calls
                }));
                    pair?;
                    if is_combined {
                        combined.push(rpc_ms);
                    } else {
                        split.push(rpc_ms);
                    }
                }
            }
            Ok(())
        }
        .await;
    let metrics = timed_call(&client, "tv_metrics", json!({}), &mut auxiliary_calls).await;
    let cleanup = timed_call(&client, "tv_disconnect", json!({}), &mut auxiliary_calls)
        .await
        .and_then(|result| require_success(&result));
    let shutdown = client.cancel().await;
    let report = json!({
        "comparison": "split versus combined MCP calls on one persistent TV session",
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "latency_unit": "milliseconds",
        "pair_rpc_ms_scope": "sum of MCP calls, excluding local PNG validation and evidence writes",
        "pair_wall_ms_scope": "MCP calls plus local PNG validation and evidence writes",
        "limitations": ["No LLM reasoning time measured", "Pixel change does not prove focus movement or semantic success", "RIGHT then LEFT returns focus only when the UI cooperates", "One TV and session; not an old one-shot connection baseline"],
        "completed": journey.is_ok(), "cleanup_confirmed": cleanup.is_ok(),
        "client_shutdown_confirmed": shutdown.is_ok(),
        "split": summary(&split), "combined": summary(&combined),
        "trials": trials, "auxiliary_calls": auxiliary_calls,
        "metrics": metrics.as_ref().ok().and_then(|result| result.structured_content.as_ref()),
        "metrics_read_success": metrics.as_ref().is_ok_and(|result| result.is_error != Some(true))
    });
    save_private(
        &evidence.join("measurements.json"),
        &serde_json::to_vec_pretty(&report)?,
    )?;
    eprintln!(
        "Split: {}; combined: {}. Inspect screenshots for actual focus behavior.",
        summary(&split),
        summary(&combined)
    );
    journey?;
    cleanup?;
    shutdown?;
    require_success(&metrics?)?;
    Ok(())
}

#[test]
#[ignore = "local CPU microbenchmark; no TV access; results depend on build mode and host load"]
fn metrics_overhead() {
    const SAMPLES: u64 = 50_000;
    let mut enabled_runs = Vec::new();
    let mut disabled_runs = Vec::new();
    for round in 0..5 {
        for enabled in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let metrics = Metrics::new(enabled);
            let started = Instant::now();
            for _ in 0..SAMPLES {
                std::hint::black_box(&metrics)
                    .begin(Operation::Observe)
                    .finish(Outcome::Success);
            }
            let ns_per_sample = started.elapsed().as_nanos() as f64 / SAMPLES as f64;
            assert_eq!(
                metrics.snapshot().operations[&Operation::Observe].count,
                if enabled { SAMPLES } else { 0 }
            );
            if enabled {
                enabled_runs.push(ns_per_sample);
            } else {
                disabled_runs.push(ns_per_sample);
            }
        }
    }
    eprintln!(
        "{} build; five alternating runs, {SAMPLES} samples each; enabled mean {:.1} ns/sample; disabled mean {:.1} ns/sample; local aggregation only, no TV or LLM time",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        enabled_runs.iter().sum::<f64>() / 5.0,
        disabled_runs.iter().sum::<f64>() / 5.0
    );
    eprintln!("Per-run ns/sample: enabled {enabled_runs:?}; disabled {disabled_runs:?}");
}
