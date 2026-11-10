//! Process wiring: choose a front end and attach it to the live TV.
mod live;
mod local_store;
mod setup;

use clap::Parser;
use rmcp::transport::stdio;
use tokio::io::BufReader;
use webos_mcp::{
    cli::{self, Calls, CallsOutcome, Cli, Command},
    metrics::Metrics,
    server::Front,
    session::{SessionOwner, StateDir},
};

/// The same started front end for MCP stdio and for the command line.
fn front() -> Front<live::LiveDevice> {
    let metrics = Metrics::new(
        std::env::var_os("WEBOS_MCP_METRICS").as_deref() != Some(std::ffi::OsStr::new("0")),
    );
    let session = SessionOwner::new(
        metrics.clone(),
        StateDir::platform(),
        live::LiveDevice::idle,
    );
    Front::start(metrics, session)
}

/// Resolves when the user or the parent process asks this process to stop.
/// If no handler can be installed, it never resolves and the default applies.
async fn interrupted() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut terminate) = signal(SignalKind::terminate()) else {
            return std::future::pending().await;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

const CLEANUP_UNCONFIRMED: &str = "TV session cleanup could not be confirmed.";

async fn run_tools(
    calls: Calls<BufReader<tokio::io::Stdin>>,
    output: cli::Output,
    keep_going: bool,
) -> Result<(), &'static str> {
    let report = cli::run_calls(
        front(),
        calls,
        output,
        keep_going,
        std::io::stdout(),
        interrupted(),
    )
    .await?;
    if !report.cleanup_confirmed {
        eprintln!("{CLEANUP_UNCONFIRMED}");
    }
    match report.calls {
        CallsOutcome::Interrupted => {
            eprintln!("Interrupted; any call in flight was cancelled.");
            std::process::exit(130);
        }
        CallsOutcome::SomeFailed => Err("A tool call failed."),
        CallsOutcome::AllSucceeded if !report.cleanup_confirmed => {
            Err("The calls succeeded, but cleanup is unconfirmed.")
        }
        CallsOutcome::AllSucceeded => Ok(()),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let outcome = match Cli::parse().command {
        None => {
            if !front().serve(stdio(), interrupted()).await? {
                eprintln!("{CLEANUP_UNCONFIRMED}");
            }
            // The session is released. Exit instead of returning: after a
            // signal, stdin is still open and the runtime would wait on its
            // blocked reader forever.
            std::process::exit(0);
        }
        Some(Command::Setup { re_pair }) => setup::setup(re_pair).await,
        Some(Command::UseLocalFile {
            import_stdin: false,
        }) => setup::migrate_local().await,
        Some(Command::UseLocalFile { import_stdin: true }) => setup::import_local().await,
        Some(Command::AcknowledgeDelivery) => setup::acknowledge_delivery(),
        Some(Command::Remove) => setup::remove(),
        Some(Command::Call { output, tool }) => {
            run_tools(Calls::One(Box::new(tool)), output, false).await
        }
        Some(Command::Session {
            output,
            keep_going,
            calls,
        }) => {
            let calls = if calls.is_empty() {
                Calls::Reader(BufReader::new(tokio::io::stdin()))
            } else {
                Calls::Lines(calls)
            };
            run_tools(calls, output, keep_going).await
        }
    };
    if let Err(message) = outcome {
        eprintln!("{message}");
        std::process::exit(1);
    }
    Ok(())
}
