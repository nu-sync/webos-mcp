# webos-mcp

Let an AI agent see and navigate your LG webOS TV.

`webos-mcp` is a small [Model Context Protocol](https://modelcontextprotocol.io) server written in Rust. It runs on your computer, talks to one TV you have paired, and gives an agent two abilities: take a screenshot, and press a remote button. The agent looks at the screen, decides what to do, and looks again.

> **Status: early.** The full loop has been tested on one rooted TV from macOS. Windows, Linux, and other TV models are unverified. See [verification status](docs/verification.md) for what has and has not been tested.

## Tools

| Tool | Purpose |
| --- | --- |
| `tv_connect` | Open a session with the paired TV and report whether screenshots are available. |
| `tv_observe` | Take a fresh screenshot and return an `observation_id` for the next action. |
| `tv_press` | Press a remote button: `UP`, `DOWN`, `LEFT`, `RIGHT`, `ENTER`, `BACK`, or `HOME`. |
| `tv_launch` | Launch an app by its app ID. |
| `tv_disconnect` | Close the session and confirm cleanup. |
| `tv_metrics` | Report local timing and outcome counts for the tools above. |

## Requirements

- An LG webOS TV on a network your computer can reach.
- Rust, to build the server.
- For screenshots: a rooted TV with SSH already enabled, and its host key already in your `known_hosts`. Without this, `tv_connect` still works but reports that visual capture is unavailable, and actions stay blocked.

`webos-mcp` never roots a TV, enables SSH, installs software, or changes TV settings. It does not scan your network; you give it the TV's address.

## Quick start

**1. Build**

```sh
cargo build --release
```

**2. Pair with the TV**

```sh
./target/release/webos-mcp setup
```

Setup asks for:

- **The TV's IP address**, from the TV's network settings.
- **The TV's TLS certificate fingerprint** (SHA-256). Get it through a channel you already trust, not only from the same unauthenticated connection. If you have a verified SSH session on a rooted TV, one way is to run this on the TV:

  ```sh
  openssl s_client -connect 127.0.0.1:3001 </dev/null 2>/dev/null | openssl x509 -noout -fingerprint -sha256
  ```

- **Optional SSH details for screenshots**: user, port, agent socket or pipe, and public key file. Press Enter at the agent prompt to use `SSH_AUTH_SOCK` from the server's environment each time it connects; a saved path that stops working also falls back to it.

Approve the pairing prompt on the TV. Credentials are saved in your operating system's credential store.

**3. Add it to your MCP client**

Point the client at the release binary, with no arguments. It speaks MCP over stdio.

```json
{
  "mcpServers": {
    "webos": { "command": "/path/to/webos-mcp/target/release/webos-mcp" }
  }
}
```

On Windows the binary is `webos-mcp.exe`.

## How an agent uses it

1. Call `tv_connect`, then `tv_observe`.
2. Look at the image. Pass its `observation_id` to `tv_press` or `tv_launch`.
3. The action returns a new screenshot and a new ID. Use that ID for the next action.
4. Call `tv_disconnect` when done.

For example, `tv_press` with `{ "button": "RIGHT", "observation_id": "<latest ID>" }`.

Keep one server process and one session open while navigating.

## Safety rules

A TV is a shared screen, and a repeated button press can do the wrong thing. The server is strict about this:

- **Look before you act.** Every action needs the ID from the latest screenshot. An old ID is rejected before anything is sent.
- **No automatic retries.** If the server cannot tell whether a button press reached the TV, it blocks further actions, even after reconnecting, rather than risk sending it twice. Only you can lift that block: look at the TV, then run `webos-mcp acknowledge-delivery` in a terminal. An agent cannot do this through MCP.
- **Sent is not the same as applied.** `delivery: sent` means the message left your computer. Check the screenshot to see what the TV did.
- **A failed screenshot never returns an old image.** If the screenshot after an action fails, do not repeat the action; call `tv_observe`.
- **A new screenshot does not mean the screen has settled.** `ui_settled` is always `unknown`.

Screenshots may leave out video, HDMI inputs, and protected content. If your agent uses a remote model, screenshots leave your machine.

The full contract, including timing options such as `wait_ms` and `change_timeout_ms`, is in [SPEC.md](SPEC.md#mcp-tools).

## Configuration

Set these in the MCP server's environment before it starts.

| Variable | Effect |
| --- | --- |
| `WEBOS_MCP_LOSSLESS=1` | Capture lossless PNG. By default the TV captures JPEG for speed and the server returns that JPEG unchanged, after decoding it once to validate it. |
| `WEBOS_MCP_METRICS=0` | Turn off metrics collection. |
| `WEBOS_MCP_DIAGNOSTICS=1` | Print connection-stage diagnostics to stderr. |

Metrics stay in memory on your computer. They hold operation names, counts, durations, and outcomes, and never screen content, device identifiers, or button names.

## Commands

| Command | What it does |
| --- | --- |
| `webos-mcp setup` | Pair with a TV and save credentials. |
| `webos-mcp setup --re-pair` | Ask the TV for a new authorization. |
| `webos-mcp remove` | Delete local credentials. This does not revoke the authorization on the TV. |
| `webos-mcp use-local-file` | Move an existing pairing into a private, unencrypted local file. |
| `webos-mcp acknowledge-delivery` | After you have checked the TV, clear a block left by an action whose delivery could not be confirmed. Asks for typed confirmation and never resends the action. |

The local file avoids repeated credential-store prompts, but anyone who can read that file, including backups, can read the TV address and pairing key. On Unix the file is readable only by you; Windows permissions have not been verified. The server never falls back to plaintext on its own.

Your operating system may ask you to unlock or approve credential access, especially after you rebuild the binary.

If `tv_disconnect` reports that cleanup could not be confirmed, the session stays closed to new work on purpose. Restart the MCP server to release it; the error says so.

## Development

These checks contact no TV and no credential store:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

The session lifecycle is also model-checked with TLA+; see [statechart/README.md](statechart/README.md). CI additionally runs `cargo deny check` against [deny.toml](deny.toml) for advisories, licenses, and sources.

Tests named `tests/live_*.rs` are ignored by default because they contact, and in some cases control, the configured TV. Read a test before running it. Those that save screenshots need `WEBOS_MCP_EVIDENCE_DIR` set to a private directory outside this repository. For example, this one connects, reports connect timings, and disconnects without pressing anything:

```sh
cargo test --release --test live_connect_stages -- --ignored --nocapture
```

More reading:

- [SPEC.md](SPEC.md): scope, tool contracts, and security boundaries.
- [docs/verification.md](docs/verification.md): what has been tested, and how.
- [AGENTS.md](AGENTS.md): guidance for contributors and coding agents.

## License

[MIT](LICENSE)
