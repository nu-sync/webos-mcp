//! Command-line access to the same tool handlers the MCP server exposes.
//!
//! `call` and `session` build the server exactly as stdio mode does and invoke
//! the same `tv_*` handler functions with the same parameter types, so a result
//! printed here is the `CallToolResult` an MCP client would receive. Only the
//! framing differs: arguments arrive as flags instead of JSON, and image bytes
//! are written to a file instead of being inlined as base64.
use crate::{
    server::{Front, LaunchParams, ObserveParams, PressParams, TvServer},
    session::Device,
};
use base64::Engine;
use clap::{Args, Parser, Subcommand};
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// Stands for the observation ID of the most recent result in this session.
const LATEST: &str = "latest";

#[derive(Parser)]
#[command(
    name = "webos-mcp",
    version,
    about = "A local MCP server for visually navigating LG webOS TVs",
    long_about = "A local MCP server for visually navigating LG webOS TVs.\n\nRun without a command to start the MCP stdio server. `call` and `session` run the same tools from a terminal."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Pair with a TV and save credentials (interactive)
    Setup {
        /// Ask the TV for a new authorization instead of reusing the saved one
        #[arg(long)]
        re_pair: bool,
    },
    /// Copy an existing pairing into a private, unencrypted local file
    UseLocalFile {
        /// Read the credential record from stdin instead of the system store
        #[arg(long)]
        import_stdin: bool,
    },
    /// After checking the TV, clear a block left by an unconfirmed action (interactive)
    AcknowledgeDelivery,
    /// Delete local credentials; this does not revoke the authorization on the TV
    Remove,
    /// Call one MCP tool in a fresh server, print its result as JSON, and exit
    ///
    /// The server starts disconnected, exactly as an MCP server does, so a lone
    /// `tv_observe` reports that the TV is disconnected. Use `session` to run a
    /// sequence against one connection.
    Call {
        #[command(flatten)]
        output: Output,
        #[command(subcommand)]
        tool: ToolCall,
    },
    /// Run tool calls against one server session, one result per output line
    ///
    /// Each call is written as it would be after `webos-mcp call`, for example
    /// `tv_press --button RIGHT --observation-id latest`. Calls come from
    /// repeated `--call` options, or from stdin (one per line; blank lines and
    /// lines starting with `#` are skipped) when none is given. The session is
    /// disconnected when the calls end.
    Session {
        #[command(flatten)]
        output: Output,
        /// Continue after a failed call instead of stopping at the first one
        #[arg(long)]
        keep_going: bool,
        /// A tool call to run; repeat for several
        #[arg(short = 'c', long = "call", value_name = "CALL")]
        calls: Vec<String>,
    },
}

#[derive(Args)]
pub struct Output {
    /// Save returned screenshots in this existing directory; without it only
    /// their type and size are reported
    #[arg(long, value_name = "DIR")]
    pub image_dir: Option<PathBuf>,
}

/// One MCP tool call. Subcommand names and options match the tool names and
/// input properties; a test compares them with the published tool schemas.
#[derive(Subcommand)]
pub enum ToolCall {
    /// Connect to the configured TV without waking it
    #[command(name = "tv_connect")]
    Connect,
    /// Capture a fresh TV image and observation ID
    #[command(name = "tv_observe")]
    Observe(ObserveParams),
    /// Send one to five paced button presses using the latest observation ID
    #[command(name = "tv_press")]
    Press(PressParams),
    /// Launch an app using the latest observation ID
    #[command(name = "tv_launch")]
    Launch(LaunchParams),
    /// Close the TV session, reporting any unconfirmed cleanup
    #[command(name = "tv_disconnect")]
    Disconnect,
    /// Read local aggregate timings and outcomes without contacting the TV
    #[command(name = "tv_metrics")]
    Metrics,
}

impl ToolCall {
    fn name(&self) -> &'static str {
        match self {
            Self::Connect => "tv_connect",
            Self::Observe(_) => "tv_observe",
            Self::Press(_) => "tv_press",
            Self::Launch(_) => "tv_launch",
            Self::Disconnect => "tv_disconnect",
            Self::Metrics => "tv_metrics",
        }
    }

    fn observation_id(&mut self) -> Option<&mut String> {
        match self {
            Self::Press(params) => Some(&mut params.observation_id),
            Self::Launch(params) => Some(&mut params.observation_id),
            _ => None,
        }
    }
}

impl<D: Device + 'static> TvServer<D> {
    /// The handler an MCP `tools/call` request for this tool reaches.
    pub async fn call(&self, tool: ToolCall) -> CallToolResult {
        match tool {
            ToolCall::Connect => self.tv_connect().await,
            ToolCall::Observe(params) => self.tv_observe(Parameters(params)).await,
            ToolCall::Press(params) => self.tv_press(Parameters(params)).await,
            ToolCall::Launch(params) => self.tv_launch(Parameters(params)).await,
            ToolCall::Disconnect => self.tv_disconnect().await,
            ToolCall::Metrics => self.tv_metrics(),
        }
    }
}

/// A session line: a tool call without the program name.
#[derive(Parser)]
#[command(no_binary_name = true, disable_help_flag = true)]
struct Line {
    #[command(subcommand)]
    tool: ToolCall,
}

fn parse_line(line: &str) -> Result<ToolCall, String> {
    let words = shlex::split(line).ok_or("unbalanced quotes in call")?;
    Line::try_parse_from(words)
        .map(|line| line.tool)
        .map_err(|error| error.render().to_string())
}

/// Where a session's calls come from. `R` is the line source for `Reader`:
/// stdin in the binary, a pipe in tests.
pub enum Calls<R> {
    One(Box<ToolCall>),
    Lines(Vec<String>),
    Reader(R),
}

struct Printer<W> {
    out: W,
    image_dir: Option<PathBuf>,
    sequence: u32,
    latest: Option<String>,
}

impl<W: Write> Printer<W> {
    /// Run one call and print its result. Returns whether the tool succeeded.
    async fn run<D: Device + 'static>(
        &mut self,
        server: &TvServer<D>,
        mut tool: ToolCall,
    ) -> Result<bool, &'static str> {
        // The caller of an MCP tool passes back the ID it was last given; a
        // script cannot know it in advance, so `latest` names it. The handler
        // still validates whatever ID arrives.
        if let (Some(id), Some(latest)) = (tool.observation_id(), &self.latest)
            && id == LATEST
        {
            id.clone_from(latest);
        }
        let name = tool.name();
        let result = server.call(tool).await;
        self.sequence += 1;
        let failed = result.is_error == Some(true);
        let mut rendered =
            serde_json::to_value(&result).map_err(|_| "Could not encode the tool result")?;
        if let Some(id) = rendered
            .pointer("/structuredContent/observation_id")
            .and_then(Value::as_str)
        {
            self.latest = Some(id.to_owned());
        }
        if let Some(blocks) = rendered.get_mut("content").and_then(Value::as_array_mut) {
            for block in blocks {
                self.externalize_image(name, block)?;
            }
        }
        let line = json!({"tool": name, "result": rendered}).to_string();
        writeln!(self.out, "{line}")
            .and_then(|_| self.out.flush())
            .map_err(|_| "Could not write the tool result")?;
        Ok(!failed)
    }

    /// Replace inline base64 image data with its size and, when an image
    /// directory was given, the private file it was saved to.
    fn externalize_image(&self, tool: &str, block: &mut Value) -> Result<(), &'static str> {
        if block.get("type").and_then(Value::as_str) != Some("image") {
            return Ok(());
        }
        let Some(Value::String(data)) = block.as_object_mut().and_then(|b| b.remove("data")) else {
            return Err("Tool result image has no data");
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| "Tool result image is not valid base64")?;
        block["bytes"] = json!(bytes.len());
        if let Some(directory) = &self.image_dir {
            let extension = match block.get("mimeType").and_then(Value::as_str) {
                Some("image/jpeg") => "jpg",
                Some("image/png") => "png",
                _ => "bin",
            };
            let path = directory.join(format!("{:03}-{tool}.{extension}", self.sequence));
            save_private(&path, &bytes)?;
            block["path"] = json!(path);
        }
        Ok(())
    }
}

fn save_private(path: &Path, bytes: &[u8]) -> Result<(), &'static str> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|_| "Could not save the screenshot; the file may already exist")
}

/// Run calls in order against one server. Returns whether every call succeeded.
/// A failed or unparsable call stops the run unless `keep_going` is set, so a
/// script never sends a later action on top of an earlier failure by accident.
async fn run<D, R, W>(
    server: &TvServer<D>,
    calls: Calls<R>,
    output: Output,
    keep_going: bool,
    out: W,
) -> Result<bool, &'static str>
where
    D: Device + 'static,
    R: AsyncBufRead + Unpin,
    W: Write,
{
    if let Some(directory) = &output.image_dir
        && !directory.is_dir()
    {
        return Err("--image-dir must be an existing directory");
    }
    let mut printer = Printer {
        out,
        image_dir: output.image_dir,
        sequence: 0,
        latest: None,
    };
    let mut all_succeeded = true;
    let mut step = async |printer: &mut Printer<W>, line: &str| -> Result<bool, &'static str> {
        let succeeded = match parse_line(line) {
            Ok(tool) => printer.run(server, tool).await?,
            Err(error) => {
                eprintln!("{error}");
                false
            }
        };
        all_succeeded &= succeeded;
        Ok(succeeded || keep_going)
    };
    match calls {
        Calls::One(tool) => return printer.run(server, *tool).await,
        Calls::Lines(lines) => {
            for line in &lines {
                if !step(&mut printer, line).await? {
                    break;
                }
            }
        }
        Calls::Reader(reader) => {
            let mut lines = reader.lines();
            while let Some(line) = lines
                .next_line()
                .await
                .map_err(|_| "Could not read calls")?
            {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if !step(&mut printer, line).await? {
                    break;
                }
            }
        }
    }
    Ok(all_succeeded)
}

/// How a run of calls ended, before cleanup.
#[derive(Debug, PartialEq, Eq)]
pub enum CallsOutcome {
    AllSucceeded,
    SomeFailed,
    Interrupted,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub calls: CallsOutcome,
    pub cleanup_confirmed: bool,
}

/// Run calls through a started front end, then release its session.
///
/// If `interrupt` resolves first, the call in flight is cancelled exactly as a
/// cancelled MCP request would be (an action mid-send stays unresolved) and
/// the session is still released.
pub async fn run_calls<D, R, W>(
    front: Front<D>,
    calls: Calls<R>,
    output: Output,
    keep_going: bool,
    out: W,
    interrupt: impl Future<Output = ()>,
) -> Result<Report, &'static str>
where
    D: Device + 'static,
    R: AsyncBufRead + Unpin,
    W: Write,
{
    let ran = tokio::select! {
        ran = run(&front.server, calls, output, keep_going, out) => Some(ran),
        () = interrupt => None,
    };
    let cleanup_confirmed = front.stop().await;
    let calls = match ran {
        Some(Ok(true)) => CallsOutcome::AllSucceeded,
        Some(Ok(false)) => CallsOutcome::SomeFailed,
        Some(Err(error)) => return Err(error),
        None => CallsOutcome::Interrupted,
    };
    Ok(Report {
        calls,
        cleanup_confirmed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{capture::CapturedImage, error::TvError, metrics::Metrics, session::Action};
    use clap::CommandFactory;
    use std::collections::{BTreeMap, BTreeSet};

    /// Names the server type; these tests reach no device.
    struct Unused;
    impl Device for Unused {
        async fn establish(&mut self, _: &Metrics) -> Result<bool, TvError> {
            unreachable!()
        }
        async fn probe(&mut self) -> Result<(), TvError> {
            unreachable!()
        }
        async fn send(&mut self, _: &Action) -> Result<(), TvError> {
            unreachable!()
        }
        async fn capture(&mut self) -> Result<CapturedImage, TvError> {
            unreachable!()
        }
        async fn close(&mut self) -> Result<(), TvError> {
            unreachable!()
        }
    }

    /// Names and option sets of the tool subcommands, as clap sees them.
    fn cli_tools() -> BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)> {
        let command = Line::command();
        command
            .get_subcommands()
            .map(|tool| {
                let options = |required: bool| {
                    tool.get_arguments()
                        .filter(|argument| !required || argument.is_required_set())
                        .map(|argument| argument.get_id().to_string())
                        .collect::<BTreeSet<_>>()
                };
                (tool.get_name().to_owned(), (options(false), options(true)))
            })
            .collect()
    }

    #[test]
    fn every_mcp_tool_has_a_cli_command_with_the_same_inputs() {
        let mcp: BTreeMap<_, _> = TvServer::<Unused>::tool_router()
            .list_all()
            .into_iter()
            .map(|tool| {
                let schema = serde_json::to_value(&tool.input_schema).unwrap();
                let names = |value: Option<&Value>| -> BTreeSet<String> {
                    match value {
                        Some(Value::Object(map)) => map.keys().cloned().collect(),
                        Some(Value::Array(list)) => list
                            .iter()
                            .map(|name| name.as_str().unwrap().to_owned())
                            .collect(),
                        _ => BTreeSet::new(),
                    }
                };
                (
                    tool.name.to_string(),
                    (
                        names(schema.get("properties")),
                        names(schema.get("required")),
                    ),
                )
            })
            .collect();
        assert_eq!(mcp.len(), 6);
        assert_eq!(cli_tools(), mcp);
    }

    #[test]
    fn a_session_line_parses_to_the_same_parameters_as_mcp_arguments() {
        let ToolCall::Press(cli) = parse_line(
            "tv_press --button RIGHT --observation-id 'abc 1' --count 2 --observe false --wait-ms 10",
        )
        .unwrap() else {
            panic!("tv_press")
        };
        let mcp: PressParams = serde_json::from_value(json!({
            "button": "RIGHT", "observation_id": "abc 1", "count": 2, "observe": false, "wait_ms": 10
        }))
        .unwrap();
        assert_eq!(format!("{cli:?}"), format!("{mcp:?}"));
        let ToolCall::Launch(cli) =
            parse_line("tv_launch --app-id example.app --observation-id latest").unwrap()
        else {
            panic!("tv_launch")
        };
        let mcp: LaunchParams =
            serde_json::from_value(json!({"app_id": "example.app", "observation_id": "latest"}))
                .unwrap();
        assert_eq!(format!("{cli:?}"), format!("{mcp:?}"));
        assert!(matches!(parse_line("tv_observe"), Ok(ToolCall::Observe(_))));
    }

    #[test]
    fn malformed_lines_are_rejected_before_any_handler_runs() {
        for line in [
            "tv_press --button SIDEWAYS --observation-id x",
            "tv_press --button RIGHT",
            "tv_press --button RIGHT --observation-id x --extra 1",
            "tv_observe --wait-ms -1",
            "tv_reboot",
            "tv_launch --app-id 'unterminated",
        ] {
            assert!(parse_line(line).is_err(), "{line}");
        }
    }

    #[test]
    fn images_are_reported_by_size_and_saved_only_when_asked() {
        let directory = tempfile::tempdir().unwrap();
        let block = || json!({"type": "image", "data": "AQID", "mimeType": "image/jpeg"});
        let mut printer = Printer {
            out: Vec::new(),
            image_dir: None,
            sequence: 7,
            latest: None,
        };
        let mut unsaved = block();
        printer
            .externalize_image("tv_observe", &mut unsaved)
            .unwrap();
        assert_eq!(
            unsaved,
            json!({"type": "image", "mimeType": "image/jpeg", "bytes": 3})
        );
        printer.image_dir = Some(directory.path().to_owned());
        let mut saved = block();
        printer.externalize_image("tv_observe", &mut saved).unwrap();
        let path = directory.path().join("007-tv_observe.jpg");
        assert_eq!(saved["path"], json!(path));
        assert_eq!(std::fs::read(&path).unwrap(), [1, 2, 3]);
        assert!(
            printer
                .externalize_image("tv_observe", &mut block())
                .is_err()
        );
        let mut text = json!({"type": "text", "text": "unchanged"});
        printer.externalize_image("tv_observe", &mut text).unwrap();
        assert_eq!(text, json!({"type": "text", "text": "unchanged"}));
    }
}
