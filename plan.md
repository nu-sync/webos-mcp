# webos-mcp implementation plan

## Goal

Build a small Rust MCP server that lets an assistant connect to a supported LG webOS TV, observe a fresh screen, send a requested remote action, observe its result, and disconnect. Pair once and reuse credentials where a secure OS store is available. The assistant owns navigation decisions.

## Scope

Follow [SPEC.md](SPEC.md) and the [TV session model](statechart/README.md). Target one selected TV and local stdio MCP clients on macOS, Windows, and Linux. Separate ordinary pairing from optional rooted SSH capture. Treat OS support and TV capabilities as unverified until tested. Exclude continuous video, TV-side installation, automatic focus recognition, power changes, purchases, and public deployment. This plan is a checkpoint; it does not itself start implementation or authorize publication.

## Completion

The six specified tools run through the real stdio MCP server; lifecycle and onboarding models match the implementation; setup and secure credential reuse work on each verified platform; screenshots are fresh, private, and honest about unsupported capture; uncertain delivery cannot cause an unsafe retry. A user-authorized TV journey demonstrates the real loop, owned resources close, and the published support matrix distinguishes tested combinations from gaps. Performance is measured without requiring a 2–3× gain.

## Unit model
Status: pending
Depends on: none
Write scope: statechart/ and model documentation
Acceptance: Model setup, interrupted setup, credential revocation, capability negotiation, and the late-delivery recovery policy; revise the TV session model to reflect the chosen safe behavior without assuming a screenshot cancels pending input.
Verification: Run TLC against each configuration; inspect generated charts, all invariant and deadlock results, and paths for a deliberately injected late-delivery fault. Document assumptions and any unmodeled OS or network behavior.
Evidence: pending

## Unit scaffold
Status: pending
Depends on: model
Write scope: Cargo manifests, Rust server entry point, MCP transport and integration tests
Acceptance: A Rust rmcp stdio process exposes the six specified tool schemas, starts disconnected, sends diagnostics only to stderr, handles EOF, returns an empty metrics snapshot while disconnected, and responds with explicit not-ready errors for TV operations until backends exist.
Verification: Run cargo fmt --check, cargo clippy, cargo test, and a real MCP client subprocess test of discovery, validation, responses, and clean shutdown; no TV interaction.
Evidence: pending

## Unit onboarding
Status: pending
Depends on: scaffold
Write scope: setup CLI, credential and configuration adapters, pairing protocol, platform tests, and user-facing setup documentation
Acceptance: User-selected TV pairing stores its client key in the available OS credential store, reuses it across process restarts, and supports inspection, re-pair and removal; errors and interruptions recover without leaking secrets or silently storing plaintext. Implement a genuine same-process session-only flow or document it as unavailable until it can be supported.
Verification: Test real CLI journeys with a controllable fake TV and isolated fake store, then verify native stores on each available target OS including restart, revoked credentials, locked/unavailable store, changed TV identity, and interrupted setup. Label untested OS and real-TV cases as gaps; scan artifacts for synthetic-versus-real credentials.
Evidence: pending

## Unit capture
Status: pending
Depends on: onboarding
Write scope: capture capability adapters, TV trust configuration, setup capability probe and capture tests
Acceptance: A successful observation returns a newly captured valid image and metadata; unsupported capture is explicit. Provide a validated optional rooted SSH path with host-key verification and user-supplied authentication, and test any undocumented unrooted path before claiming support. Prevent stale images, unsafe returned URLs, and secret/image logging.
Verification: Exercise success, stale-frame prevention, complete image validation, SSH input lifetime, timeout, URL restrictions, size bounds, identity changes, and owned-file cleanup with fake endpoints; verify the supported real TV backend only when access and a non-disruptive observation are available. Check portability of the selected SSH and credential adapters on each target OS.
Evidence: pending

## Unit session
Status: pending
Depends on: capture
Write scope: session owner, remote and input transport adapters, lifecycle and fault-injection tests
Acceptance: One persistent task-scoped session enforces the checked model, serializes TV actions, rejects invalid or stale operations, invalidates observations after actions, closes only owned resources, and blocks unsafe action retries after ambiguous delivery. Input batches are bounded and paced.
Verification: Drive the actual session API against a controllable fake transport, testing every modeled transition, separate control and input socket failures, tunnel port conflicts, in-flight disconnect, late reply, timeout, reconnect and graceful shutdown; compare traces to the checked model. Verify the late-delivery gate from the model before allowing recovery; report any unresolved guarantee as a blocker.
Evidence: pending

## Unit metrics
Status: pending
Depends on: session
Write scope: in-memory metrics, stage timers, telemetry settings and fake-transport tests
Acceptance: Bounded aggregate-only metrics count and time completed operations and fixed stages, include disjoint outcomes and cancellation, and expose no private content; disabling collection leaves session behavior unchanged. Snapshots never perform TV I/O or include incomplete operations.
Verification: Inject known delays, failures, timeouts, cancellation and skipped stages into a fake transport; verify timing bounds, exact counts, privacy allowlists and snapshots during in-flight calls. Compare enabled and disabled behavior and measure overhead without claiming an unmeasured threshold.
Evidence: pending

## Unit integration
Status: pending
Depends on: metrics
Write scope: MCP tool handlers, end-to-end fake-device tests, and setup/help documentation
Acceptance: The real compiled stdio server implements connect, observe, press, launch, disconnect, and read-only metrics with typed schemas, native image results, explicit busy and uncertain errors, and no secrets in logs. MCP server and TV session lifetimes remain separate.
Verification: Launch the compiled server with an MCP client against a fake device; verify image MIME and bytes, results and error semantics, multiple calls reusing connections, metrics available while disconnected, EOF shutdown, and visible resource cleanup. Validate the client actually displays returned images when that client is available.
Evidence: pending

## Unit validation
Status: pending
Depends on: integration
Write scope: verification harness, support matrix, non-sensitive evidence and public documentation
Acceptance: An authorized real-TV navigation reaches and visually verifies a specified target using a single session; actual cleanup and a reproducible cross-platform support matrix are documented. Public contents and any future Git history contain no user-specific details, credentials or private screenshots.
Verification: Perform a real requested journey only with the TV available and authorized; inspect fresh before/after images, verify owned connections close, and report capability gaps. Run format, clippy, tests, MCP integration and TLC. Review repository files and staged changes with a secret scanner when available and manual inspection; do not commit raw environment logs or screenshots. Check each supported OS with real native credentials or explicitly record missing coverage.
Evidence: pending

## Unit performance
Status: pending
Depends on: validation
Write scope: benchmark harness and non-sensitive performance report
Acceptance: Compare the existing one-shot control workflow to the persistent session on an equivalent authorized journey; report completion, failures, retries, median and range for each stage and total. Treat 2–3× end-to-end improvement as a stretch target, not a claimed outcome.
Verification: Run at least five matched, alternating trials per candidate, identify whether each measurement includes model inference and app loading, preserve only non-sensitive evidence, and explain the measured limiter or report it as unknown.
Performance: Baseline unknown; measure connection, actions, capture, model/tool turns and full journey; reject performance changes that reduce verified task success or weaken lifecycle safety.
Evidence: pending
