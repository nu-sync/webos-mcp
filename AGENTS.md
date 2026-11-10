# AGENTS.md

`webos-mcp` is a local Rust MCP server for visually navigating LG webOS TVs. Build a small, maintainable tool that works with existing TV pairing and reports what it can actually observe.

## Start here

- [SPEC.md](SPEC.md) defines the MVP, six MCP tools, onboarding, security boundaries, and acceptance criteria.
- [plan.md](plan.md) orders the implementation and verification work. Update a unit's status and link evidence as work progresses.
- [statechart/README.md](statechart/README.md) explains the TV session model, its assumptions, and how to rerun TLC.

The Rust stdio server has a live-tested control and rooted-SSH capture path on one TV. Aggregate metrics and combined action-observation calls are implemented; portability, live recovery, and several failure and onboarding checks remain unfinished; see [verification status](docs/verification.md). Treat OS support and TV capabilities as unverified until exercised.

## Commands and ownership

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release
```

Use the optimized binary for normal MCP operation and performance measurements. Keep stdout exclusively for MCP protocol messages; send diagnostics to stderr. Run setup as a separate interactive command, never inside the stdio protocol.

- `src/main.rs`: process wiring only; picks a front end, attaches the live device, and maps signals to an interrupt.
- `src/server.rs`: tool schemas, input validation, result formatting, operation metrics, and `Front`, the start/serve/stop lifecycle both front ends share. The parameter structs define the MCP schema and the CLI options together. Generic over the device.
- `src/cli.rs`: the clap command line. `call` and `session` reach the same `tv_*` handlers as MCP; add a tool to both or the drift test fails. Keep stdout to result lines.
- `src/live.rs`: the live TV device; the only code that joins credentials, transport, and capture.
- `src/session.rs`: observation authority, action delivery, request arbitration, and lifecycle ownership, over any `Device` and an injectable state directory.
- `src/transport.rs`: pinned control and button connections.
- `src/capture.rs`: trusted SSH capture, bounded recovery, image validation, and capture-stage metrics.
- `src/setup.rs` and `src/local_store.rs`: onboarding and credential persistence.
- `src/error.rs`: the shared error type; a fixed message plus a typed timeout or failure outcome.
- `src/metrics.rs`: bounded, in-memory aggregates with fixed labels.
- `tests/stdio.rs`: compiled-server protocol checks. `tests/cli_equivalence.rs` runs the same calls through the compiled binary's MCP and CLI front ends and compares results. `tests/e2e_simulated.rs` does the same for connected journeys, interrupts, and acknowledgement against the simulated TV in `tests/support/simulated_tv.rs`; label its results simulated. `tests/live_*.rs` are opt-in checks that can contact or control the configured TV; inspect each test before running it.

## Design and lifecycle

- Use Rust, `rmcp`, and local stdio for the MVP. Keep the MCP server lifecycle separate from the task-scoped TV session. Keep the agent responsible for navigation decisions.
- Implement the checked lifecycle as explicit states. Require the latest `observation_id` before an action; reject stale IDs before probing or sending. Actions return a new observation by default and consume the previous ID. A fresh frame, pixel change, or transport send completion does not prove the requested screen appeared or the UI settled.
- A timed-out action has uncertain delivery. Do not automatically retry or assume a new screenshot prevents late delivery. Follow the implementation gate in `SPEC.md`; update and check the model when lifecycle behavior changes.
- Keep control pairing separate from screenshot capability. Do not assume a TV is rooted or that compositor capture includes video, HDMI, or protected content. Report unsupported or unverified capabilities clearly.
- Prefer one owner for the session and its resources. Reject concurrent or stale actions, and clean up only connections, tunnels, and temporary files created by the task. Keep failed or cancelled cleanup in `Closing`; never report a clean disconnect merely because close was requested. Separate action delivery from observation errors so a failed screenshot cannot cause a repeated button.
- Preserve the separate user-request gate and device-state lock. Reject overlapping user requests; let the active request wait for a bounded internal keepalive, then recheck health and observation identity. A keepalive must not make ordinary navigation or disconnect fail as busy.
- Retry capture only under the typed recovery policy: a pre-command channel-open failure may close and reconnect once when cleanup is confirmed. Known failed setup and interrupted setup have different cleanup guarantees. Never recover by replaying an action or clearing the durable delivery guard. The guard is cleared only by the interactive `acknowledge-delivery` command after a person confirms; keep that path out of every MCP tool.
- Pixel-change sampling is bounded to four captures. `change_timeout_ms` limits when another sample may start; it is not a whole-operation deadline. Keep `ui_settled` unknown and report black frames honestly. Sampling and recovery must not return an old image after failure.

## Engineering decisions

Keep changes simple and follow existing conventions. Classify timeouts with `TvError::timeout` at the deadline, not by matching message text. **Model the Domain** means lifecycle and observation authority belong to explicit state transitions. **Fix Root Causes** means preserve regression tests for the demonstrated failure instead of adding blind retries. **Explain the Number** means distinguish measured RPC time, capture stages, and unmeasured model reasoning. **Prove It Works** means exercise the actual session or compiled MCP interface, then inspect images for claims about visible behavior.

Metrics must contain only fixed operation/stage names, counts, durations, and outcome categories. Never add addresses, app IDs, button sequences, screen contents, credentials, or raw errors as labels. Preserve `WEBOS_MCP_METRICS=0`, cancellation accounting, and metrics reads that do not wait for TV I/O.

## Onboarding and privacy

This repository is intended to be public. Never commit credentials, pairing keys, private keys, real device identifiers or addresses, private screenshots, local absolute paths, or raw logs containing environment details. Use synthetic fixtures and review staged content before publication.

Pairing should persist through the host OS credential store by default; an explicitly chosen private, unencrypted local file is also permitted. Never silently fall back to plaintext. Handle unavailable or locked stores and revoked pairing explicitly. Verify TV identity and SSH host keys; do not disable verification to make a connection succeed. Do not send credentials to an MCP client or model. Screenshots sent to a remote model leave the user's machine.

Support macOS, Windows, and Linux without hard-coded local paths or a specific credential manager. Record which platforms and TV models were actually tested. Ask the user for the TV address; do not discover devices or scan IP addresses or ports. Setup must not root a TV, enable SSH, install software, or change TV settings as a side effect.

## Verification

Follow the relevant unit in `plan.md`. Use fake devices and credential stores for repeatable failure and lifecycle tests, and label simulated evidence. For user-authorized real-TV checks, inspect the actual screen, account for the shared device, and verify cleanup. Do not equate a successful API response with a visible result.

Run the format, strict Clippy, and all-target test commands above for code changes. Exercise the compiled server with an MCP client for protocol changes. Rerun TLC and the relevant negative controls when changing lifecycle rules or models; model checking supplements implementation tests. Preserve coverage for stale and previous-session IDs, cancellation, capture failure after a successful send, uncertain delivery, cleanup failure, and keepalive contention.

Live checks require user authorization and an appropriate screen state. Preserve the user's chosen credential backend; do not trigger OS-store migrations or repeated password prompts as part of routine testing. `WEBOS_MCP_EVIDENCE_DIR` must point to a private directory outside the repository. See [README.md](README.md) for the exact live journeys and benchmark commands.

For performance work, compare matched alternating trials and report failures, build mode, sample count, median/range, and measurement limits. Fewer MCP calls do not by themselves prove a faster full agent loop. The 2–3× improvement remains a target, not a fact. Update `docs/verification.md` with generic evidence and keep private artifacts outside Git.

Do not commit raw TLC logs, screenshots, or other private evidence. Run a secret scan when available and inspect the files and Git history that will be published. A clean scan is not proof that all sensitive information is absent.
