# webos-mcp

A local Rust MCP server for a paired webOS TV. This is an early implementation: see [verification status](docs/verification.md) for the tested device path and remaining gaps.

## Setup

Run `cargo run -- setup` in a terminal. Enter the TV's IP address from its network settings. No discovery or network scanning is performed. The address, authorization, certificate fingerprint, and optional SSH configuration are stored in the operating system credential store by default.

Setup asks for the TV's SHA-256 TLS certificate fingerprint. Obtain and verify it using a trusted independent channel; do not trust a fingerprint obtained only from the same unauthenticated network connection. For an existing, independently verified SSH connection to a rooted TV, one possible method is to run this on the TV over that connection:

```sh
openssl s_client -connect 127.0.0.1:3001 </dev/null 2>/dev/null | openssl x509 -noout -fingerprint -sha256
```

Confirm the fingerprint through your trusted channel and enter it at the hidden prompt. Approve TV and OS credential-store requests when needed. For rooted SSH capture, provide an existing SSH user, port, agent socket or pipe, and public key file. The TV host key must already be trusted in the user's SSH `known_hosts` for the selected address and port. Setup does not enable SSH or change TV software. Compositor capture may omit video and external inputs.

Run `webos-mcp use-local-file` to explicitly migrate an existing pairing into an unencrypted local file outside the repository. This avoids Keychain access after migration; local users and backups with access to that file may be able to read the address and pairing key. Unix permissions are restricted to the current user; Windows access-control behavior has not been verified. The original system-store entries remain until explicitly removed.

Run `webos-mcp setup --re-pair` to request a new TV authorization and `webos-mcp remove` to remove local TV credentials. Local removal does not revoke TV-side authorization. If action delivery is uncertain, the local safety marker remains in place; no automatic recovery is offered.

## Navigation

Build the server with `cargo build --release` and configure the MCP client to launch the resulting `target/release/webos-mcp` binary (`webos-mcp.exe` on Windows) with no arguments. The optimized build substantially reduced local PNG validation time in the tested loop; capture/transfer remained the largest measured stage.

By default the TV captures JPEG for speed; the server validates it and returns a PNG containing the decoded, lossy pixels. To capture losslessly, set `WEBOS_MCP_LOSSLESS=1` in the MCP server's environment before starting it. If the TV explicitly rejects JPEG as an unsupported format, the server falls back to native PNG. A screenshot may still omit protected video or external inputs.

Start the server without arguments from an MCP client using stdio. Keep one server process and TV session open while navigating:

1. Call `tv_connect`, then `tv_observe`.
2. Inspect the image and copy its `observation_id` into the next `tv_press` or `tv_launch` call.
3. Inspect the returned image and use its new ID for the following action.
4. Call `tv_disconnect` when finished.

For example, pass `{ "button": "RIGHT", "observation_id": "<latest ID>" }` to `tv_press`. Both action tools return a new screenshot by default. An ID is valid only for the latest successful observation in the current session; a stale ID is rejected before sending. Set `observe: false` only when you will call `tv_observe` separately before another action.

Action calls default to a 250 ms wait before capture and a 1000 ms window for starting follow-up captures when the pixels are unchanged. Standalone `tv_observe` defaults to zero for both. Override these with `wait_ms` (0–5000) and `change_timeout_ms` (0–3000). Each observation takes at most four samples. The change window is not an overall operation deadline: an in-flight capture can finish after it. Pixel change, unchanged pixels, or a delay does not prove the UI has settled; `ui_settled` remains `unknown`.

An action response separates `delivery` from `observation_error`. `delivery: sent` means the transport completed, not that the TV applied the action. If its screenshot fails, do not repeat the action; request another observation. Unknown delivery blocks further actions, including after reconnection. Capture may replace one failed connection and retry within the same call only when the failure occurred before a remote capture command and closure was confirmed. Unconfirmed cleanup remains visible and blocks unsafe recovery.

## Metrics

`tv_metrics` returns local, in-memory operation and stage counts, outcomes, and timing aggregates. It records no screen content, device identifiers, button names, or payloads. Set `WEBOS_MCP_METRICS=0` before starting the server to disable collection. Metrics cover calls that enter a tool handler; malformed JSON or typed parameter rejection before a handler is excluded. These timings do not include the client's or model's work. See [the specification](SPEC.md#local-metrics) for stage coverage and [verification status](docs/verification.md) for measurement limits.

## Credential storage

The address and key are held in the selected credential store. New interactive setup currently uses the system credential store; the local-file option requires an existing pairing or a trusted structured import. The non-secret action safety marker lives in the user's local application data directory; it contains only the pending or clear state. Older development builds used Keychain for this marker, so the first connection after updating may request one additional access to migrate it. A persistent session avoids repeated Keychain reads during button presses. Setup combines the address and pairing data into one protected entry; an older record is combined at the next successful connection. The OS may still ask to unlock or authorize access, particularly after rebuilding or replacing an executable.

## Repeatable checks

Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
`cargo test --all-targets` for checks that do not contact a TV or credential store.
The lifecycle checks are described in [statechart/README.md](statechart/README.md).

When the configured TV is awake on a static menu, set `WEBOS_MCP_EVIDENCE_DIR`
to an existing private directory outside this repository, then deliberately run:

```sh
cargo test --test live_mcp -- --ignored --nocapture
cargo test --test live_performance compare_split_and_combined_control_pairs -- --ignored --nocapture
```

The first journey presses Home and Left. The comparison sends five alternating
trials per candidate, each pressing Right then Left. It saves private screenshots
and measurements; inspect the images to verify focus behavior. These commands
control the shared TV and stop on uncertainty or capture failure. The comparison
measures MCP latency without LLM reasoning and does not establish a 2–3× speedup.

A separate CPU-only aggregation check is available without TV access:

```sh
cargo test --test live_performance metrics_overhead -- --ignored --nocapture
```
