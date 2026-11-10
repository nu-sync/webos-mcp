//! Connected journeys through both front ends against a simulated TV.
//!
//! MCP runs the real protocol over an in-memory pipe; the CLI runs its real
//! parser and runner. Both use `Front`, the type the binary starts, so only
//! the device and the transport are substituted. This is simulated evidence:
//! it says nothing about a real TV's behavior.
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use base64::Engine;
use rmcp::{ServiceExt, model::CallToolRequestParams};
use serde_json::{Value, json};
use support::simulated_tv::{
    FAIL_NEXT_CAPTURE, HANG_NEXT_CAPTURE, HANGING_BUTTON, Shared, SimulatedTv, UNCERTAIN_BUTTON,
    simulated_tv,
};
use tokio::io::{AsyncWriteExt, BufReader};
use webos_mcp::{
    cli::{Calls, CallsOutcome, Output, Report, run_calls},
    metrics::Metrics,
    server::Front,
    session::{Acknowledgement, SessionOwner, StateDir, acknowledge_delivery, acquire_lock},
};

mod support;

type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
type NoReader = BufReader<tokio::io::Empty>;

/// One tool call, written once and rendered for each front end.
struct Step(&'static str, Value);

impl Step {
    /// What an agent sends: `latest` is the ID it was most recently given.
    fn mcp(&self, latest: Option<&str>) -> CallToolRequestParams {
        let mut arguments = self.1.as_object().unwrap().clone();
        if let (Some(id), Some(latest)) = (arguments.get_mut("observation_id"), latest)
            && id == "latest"
        {
            *id = json!(latest);
        }
        CallToolRequestParams::new(self.0).with_arguments(arguments)
    }

    /// What a script writes: the CLI resolves `latest` itself.
    fn cli(&self) -> String {
        let mut line = self.0.to_owned();
        for (name, value) in self.1.as_object().unwrap() {
            let value = match value {
                Value::String(text) => shlex::try_quote(text).unwrap().into_owned(),
                other => other.to_string(),
            };
            line.push_str(&format!(" --{} {value}", name.replace('_', "-")));
        }
        line
    }
}

fn press(button: &str, id: &str) -> Step {
    Step("tv_press", json!({"button": button, "observation_id": id}))
}

fn launch(app: &str) -> Step {
    Step(
        "tv_launch",
        json!({"app_id": app, "observation_id": "latest"}),
    )
}

fn call(tool: &'static str) -> Step {
    Step(tool, json!({}))
}

fn start(state: &Path) -> (Front<SimulatedTv>, Shared) {
    let (make, tv) = simulated_tv();
    let metrics = Metrics::new(true);
    let session = SessionOwner::new(metrics.clone(), StateDir::at(state.to_owned()), make);
    (Front::start(metrics, session), tv)
}

/// What a journey produced: comparable results, returned images, TV inputs.
#[derive(Debug, PartialEq)]
struct Run {
    results: Vec<Value>,
    images: Vec<Vec<u8>>,
    tv: Vec<String>,
    marker: Vec<u8>,
}

async fn through_mcp(steps: &[Step], state: &Path) -> TestResult<Run> {
    let (front, tv) = start(state);
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let served = tokio::spawn(front.serve(server_io, std::future::pending()));
    let client = ().serve(client_io).await?;
    let (mut results, mut images, mut latest) = (Vec::new(), Vec::new(), None::<String>);
    for step in steps {
        let result = client.call_tool(step.mcp(latest.as_deref())).await?;
        for image in result.content.iter().filter_map(|block| block.as_image()) {
            images.push(base64::engine::general_purpose::STANDARD.decode(&image.data)?);
        }
        let result = serde_json::to_value(result)?;
        if let Some(id) = result.pointer("/structuredContent/observation_id") {
            latest = id.as_str().map(str::to_owned);
        }
        results.push(support::comparable_session(step.0, result));
    }
    client.cancel().await?;
    assert!(served.await??, "MCP front end must confirm cleanup");
    Ok(Run {
        results,
        images,
        tv: tv.lock().unwrap().events.clone(),
        marker: std::fs::read(state.join("action-state"))?,
    })
}

fn parse_cli_output(output: &[u8]) -> TestResult<(Vec<Value>, Vec<Vec<u8>>)> {
    let (mut results, mut images) = (Vec::new(), Vec::new());
    for line in std::str::from_utf8(output)?.lines() {
        let mut printed: Value = serde_json::from_str(line)?;
        let tool = printed["tool"].as_str().ok_or("no tool name")?.to_owned();
        let result = printed["result"].take();
        for block in result["content"].as_array().into_iter().flatten() {
            if block["type"] == "image" {
                let bytes = std::fs::read(block["path"].as_str().ok_or("image not saved")?)?;
                assert_eq!(block["bytes"], json!(bytes.len()));
                images.push(bytes);
            }
        }
        results.push(support::comparable_session(&tool, result));
    }
    Ok((results, images))
}

async fn through_cli(steps: &[Step], state: &Path, images: &Path) -> TestResult<(Run, Report)> {
    let (front, tv) = start(state);
    let mut output = Vec::new();
    let report = run_calls(
        front,
        Calls::<NoReader>::Lines(steps.iter().map(Step::cli).collect()),
        Output {
            image_dir: Some(images.to_owned()),
        },
        true,
        &mut output,
        std::future::pending(),
    )
    .await?;
    let (results, images) = parse_cli_output(&output)?;
    let run = Run {
        results,
        images,
        tv: tv.lock().unwrap().events.clone(),
        marker: std::fs::read(state.join("action-state"))?,
    };
    Ok((run, report))
}

fn directories() -> TestResult<(tempfile::TempDir, PathBuf, PathBuf, PathBuf)> {
    let root = tempfile::tempdir()?;
    let paths = ["mcp-state", "cli-state", "cli-images"].map(|name| root.path().join(name));
    for path in &paths {
        std::fs::create_dir(path)?;
    }
    let [mcp, cli, images] = paths;
    Ok((root, mcp, cli, images))
}

#[tokio::test]
async fn a_connected_journey_gives_the_same_results_images_and_tv_inputs() -> TestResult<()> {
    let steps = [
        call("tv_connect"),
        call("tv_connect"),
        call("tv_observe"),
        press("RIGHT", "latest"),
        press("LEFT", "0123456789abcdef0123456789abcdef"),
        Step(
            "tv_press",
            json!({"button": "DOWN", "observation_id": "latest", "count": 3, "observe": false}),
        ),
        press("UP", "latest"),
        Step("tv_observe", json!({"wait_ms": 10, "change_timeout_ms": 0})),
        launch(FAIL_NEXT_CAPTURE),
        call("tv_observe"),
        launch("example.app"),
        press(UNCERTAIN_BUTTON, "latest"),
        call("tv_observe"),
        press("HOME", "latest"),
        call("tv_metrics"),
        call("tv_disconnect"),
        call("tv_connect"),
        call("tv_disconnect"),
    ];
    let (_root, mcp_state, cli_state, cli_images) = directories()?;
    let mcp = through_mcp(&steps, &mcp_state).await?;
    let (cli, report) = through_cli(&steps, &cli_state, &cli_images).await?;

    assert_eq!(cli.results.len(), steps.len());
    for ((step, cli), mcp) in steps.iter().zip(&cli.results).zip(&mcp.results) {
        assert_eq!(cli, mcp, "{}", step.cli());
    }
    assert_eq!(cli, mcp);
    assert_eq!(
        report,
        Report {
            calls: CallsOutcome::SomeFailed,
            cleanup_confirmed: true
        }
    );

    // Equal is not enough: both must also be what the journey intends.
    let text = |index: usize| mcp.results[index]["content"][0]["text"].clone();
    let meta = |index: usize| &mcp.results[index]["structuredContent"];
    assert_eq!(
        text(0),
        "Connected; capture is configured. Observe before acting."
    );
    assert_eq!(text(1), "TV is already connected");
    assert_eq!(meta(2)["visual_change"], "first");
    assert_eq!(meta(3)["delivery"], "sent");
    assert_eq!(meta(3)["visual_change"], "changed");
    assert_eq!(meta(4)["delivery"], "not_sent");
    assert_eq!(
        meta(4)["action_error"],
        "Observation ID is stale; observe before acting"
    );
    assert_eq!(meta(5)["delivery"], "sent");
    assert!(
        meta(5).get("width").is_none(),
        "observe=false returns no frame"
    );
    assert_eq!(meta(6)["action_error"], "Observe the TV before an action");
    assert_eq!(meta(8)["delivery"], "sent");
    assert_eq!(meta(8)["observation_error"], "Capture transfer failed");
    assert_eq!(meta(8)["do_not_repeat_action"], true);
    assert_eq!(meta(10)["delivery"], "sent");
    assert_eq!(meta(11)["delivery"], "unknown");
    assert_eq!(meta(12)["delivery_unresolved"], true);
    assert_eq!(meta(13)["delivery"], "not_sent");
    assert_eq!(meta(14)["operations"]["tv_press"]["count"], 6);
    assert_eq!(meta(14)["operations"]["tv_press"]["outcomes"]["timeout"], 1);
    assert_eq!(text(15), "TV disconnected");
    assert!(text(16).as_str().unwrap().contains("unresolved delivery"));
    assert_eq!(mcp.images.len(), 6);
    assert_ne!(mcp.images[0], mcp.images[1], "RIGHT must change the frame");
    assert_eq!(mcp.marker, b"P");
    assert_eq!(
        mcp.tv,
        [
            "establish",
            "capture",
            "press RIGHT x1",
            "capture",
            "press DOWN x3",
            "capture",
            "launch sim.fail-next-capture",
            "capture failed",
            "capture",
            "launch example.app",
            "capture",
            "press BACK x1",
            "capture",
            "close",
            "establish",
            "close",
        ]
    );
    Ok(())
}

#[tokio::test]
async fn acknowledgement_unblocks_both_front_ends_the_same_way() -> TestResult<()> {
    let blocked = [
        call("tv_connect"),
        call("tv_observe"),
        press(UNCERTAIN_BUTTON, "latest"),
        call("tv_disconnect"),
    ];
    let after = [
        call("tv_connect"),
        call("tv_observe"),
        press("RIGHT", "latest"),
        call("tv_disconnect"),
    ];
    let (_root, mcp_state, cli_state, cli_images) = directories()?;
    through_mcp(&blocked, &mcp_state).await?;
    through_cli(&blocked, &cli_state, &cli_images).await?;
    for state in [&mcp_state, &cli_state] {
        let state = StateDir::at(state.clone());
        let lock = acquire_lock(&state)?;
        assert_eq!(
            acknowledge_delivery(&state, &lock, || Ok(true))?,
            Acknowledgement::Cleared
        );
    }
    let mcp = through_mcp(&after, &mcp_state).await?;
    // A second session needs its own image directory: saved screenshots are never overwritten.
    let later_images = cli_images.with_file_name("cli-images-after");
    std::fs::create_dir(&later_images)?;
    let (cli, report) = through_cli(&after, &cli_state, &later_images).await?;
    assert_eq!(cli, mcp);
    assert_eq!(report.calls, CallsOutcome::AllSucceeded);
    assert_eq!(mcp.results[2]["structuredContent"]["delivery"], "sent");
    assert_eq!(mcp.marker, b"C");
    Ok(())
}

async fn wait_for(tv: &Shared, event: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !tv.lock().unwrap().events.iter().any(|seen| seen == event) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the TV never saw `{event}`"));
}

/// Interrupt each front end while `last` is in flight; return the TV's inputs
/// and the durable marker afterwards.
async fn interrupted(last: Step, waiting_on: &str) -> TestResult<[(Vec<String>, Vec<u8>); 2]> {
    let steps = [call("tv_connect"), call("tv_observe"), last];
    let (_root, mcp_state, cli_state, cli_images) = directories()?;

    let (front, tv) = start(&mcp_state);
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (interrupt, interrupted) = tokio::sync::oneshot::channel::<()>();
    let served = tokio::spawn(front.serve(server_io, async {
        let _ = interrupted.await;
    }));
    let client = ().serve(client_io).await?;
    let mut latest = None::<String>;
    for step in &steps[..2] {
        let result = serde_json::to_value(client.call_tool(step.mcp(None)).await?)?;
        latest = result
            .pointer("/structuredContent/observation_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    let request = steps[2].mcp(latest.as_deref());
    let in_flight = tokio::spawn(async move { client.call_tool(request).await });
    wait_for(&tv, waiting_on).await;
    interrupt.send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), served).await???,
        "an interrupted MCP server must still confirm cleanup"
    );
    assert!(
        in_flight.await?.is_err(),
        "the interrupted call gets no result"
    );
    let mcp = (
        tv.lock().unwrap().events.clone(),
        std::fs::read(mcp_state.join("action-state"))?,
    );

    let (front, tv) = start(&cli_state);
    let (interrupt, interrupted) = tokio::sync::oneshot::channel::<()>();
    let watcher = tv.clone();
    let event = waiting_on.to_owned();
    tokio::spawn(async move {
        wait_for(&watcher, &event).await;
        interrupt.send(()).unwrap();
    });
    let mut output = Vec::new();
    let report = tokio::time::timeout(
        Duration::from_secs(5),
        run_calls(
            front,
            Calls::<NoReader>::Lines(steps.iter().map(Step::cli).collect()),
            Output {
                image_dir: Some(cli_images),
            },
            false,
            &mut output,
            async {
                let _ = interrupted.await;
            },
        ),
    )
    .await??;
    assert_eq!(
        report,
        Report {
            calls: CallsOutcome::Interrupted,
            cleanup_confirmed: true
        }
    );
    assert_eq!(
        parse_cli_output(&output)?.0.len(),
        2,
        "the cut call prints nothing"
    );
    let cli = (
        tv.lock().unwrap().events.clone(),
        std::fs::read(cli_state.join("action-state"))?,
    );
    Ok([mcp, cli])
}

#[tokio::test]
async fn an_interrupt_during_capture_releases_the_session_in_both_front_ends() -> TestResult<()> {
    let [mcp, cli] = interrupted(launch(HANG_NEXT_CAPTURE), "capture hung").await?;
    assert_eq!(cli, mcp);
    assert_eq!(
        mcp.0,
        [
            "establish",
            "capture",
            "launch sim.hang-next-capture",
            "capture hung",
            "close"
        ]
    );
    assert_eq!(mcp.1, b"C", "the send completed, so nothing is unresolved");
    Ok(())
}

#[tokio::test]
async fn an_interrupt_during_send_keeps_the_delivery_guard_in_both_front_ends() -> TestResult<()> {
    let event = format!("press {HANGING_BUTTON} x1");
    let [mcp, cli] = interrupted(press(HANGING_BUTTON, "latest"), &event).await?;
    assert_eq!(cli, mcp);
    assert_eq!(mcp.0, ["establish", "capture", event.as_str(), "close"]);
    assert_eq!(mcp.1, b"P", "an interrupted send must stay unresolved");
    Ok(())
}

/// A writer that hands each printed line to the test as it is flushed.
struct Lines(tokio::sync::mpsc::UnboundedSender<String>, Vec<u8>);

impl Write for Lines {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.1.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let text = String::from_utf8(std::mem::take(&mut self.1)).unwrap();
        for line in text.lines() {
            let _ = self.0.send(line.to_owned());
        }
        Ok(())
    }
}

#[tokio::test]
async fn a_piped_session_answers_each_line_so_a_driver_can_use_real_ids() -> TestResult<()> {
    let (_root, _, state, images) = directories()?;
    let (front, tv) = start(&state);
    let (mut to_session, session_input) = tokio::io::duplex(4096);
    let (printed, mut results) = tokio::sync::mpsc::unbounded_channel();
    let session = tokio::spawn(run_calls(
        front,
        Calls::Reader(BufReader::new(session_input)),
        Output {
            image_dir: Some(images),
        },
        false,
        Lines(printed, Vec::new()),
        std::future::pending(),
    ));
    let mut ask = async |line: String| -> TestResult<Value> {
        to_session.write_all(format!("{line}\n").as_bytes()).await?;
        let answer = tokio::time::timeout(Duration::from_secs(5), results.recv())
            .await?
            .ok_or("session ended early")?;
        Ok(serde_json::from_str(&answer)?)
    };
    ask("tv_connect".into()).await?;
    let observed = ask("# a comment is skipped\ntv_observe".into()).await?;
    let id = observed["result"]["structuredContent"]["observation_id"]
        .as_str()
        .ok_or("no observation ID")?
        .to_owned();
    let pressed = ask(format!("tv_press --button RIGHT --observation-id {id}")).await?;
    assert_eq!(pressed["result"]["structuredContent"]["delivery"], "sent");
    let reused = ask(format!(
        "tv_press --button RIGHT --observation-id {id} --observe false"
    ))
    .await;
    drop(to_session);
    let report = session.await??;
    // The reused ID is rejected, which ends a session that is not told to keep going.
    assert_eq!(
        reused?["result"]["structuredContent"]["action_error"],
        "Observation ID is stale; observe before acting"
    );
    assert_eq!(report.calls, CallsOutcome::SomeFailed);
    assert!(report.cleanup_confirmed);
    assert_eq!(
        tv.lock().unwrap().events,
        ["establish", "capture", "press RIGHT x1", "capture", "close"]
    );
    Ok(())
}
