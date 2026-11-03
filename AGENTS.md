# AGENTS.md

`webos-mcp` is a proposed local Rust MCP server for visually navigating LG webOS TVs. Build a small, maintainable tool that works with existing TV pairing and reports what it can actually observe.

## Start here

- [SPEC.md](SPEC.md) defines the MVP, six MCP tools, onboarding, security boundaries, and acceptance criteria.
- [plan.md](plan.md) orders the implementation and verification work. Update a unit's status and link evidence as work progresses.
- [statechart/README.md](statechart/README.md) explains the TV session model, its assumptions, and how to rerun TLC.

The repository currently contains a specification and model, not a Rust implementation. Treat commands, OS support, and TV capabilities as unverified until exercised.

## Design and lifecycle

- Use Rust, `rmcp`, and local stdio for the MVP. Keep the MCP server lifecycle separate from the task-scoped TV session. Keep the agent responsible for navigation decisions.
- Implement the checked lifecycle as explicit states. A successful fresh observation is required before an action and after each action. A button acknowledgement does not prove the requested screen appeared.
- A timed-out action has uncertain delivery. Do not automatically retry or assume a new screenshot prevents late delivery. Follow the implementation gate in `SPEC.md`; update and check the model when lifecycle behavior changes.
- Keep control pairing separate from screenshot capability. Do not assume a TV is rooted or that compositor capture includes video, HDMI, or protected content. Report unsupported or unverified capabilities clearly.
- Prefer one owner for the session and its resources. Reject concurrent or stale actions, and clean up only connections, tunnels, and temporary files created by the task.

## Onboarding and privacy

This repository is intended to be public. Never commit credentials, pairing keys, private keys, real device identifiers or addresses, private screenshots, local absolute paths, or raw logs containing environment details. Use synthetic fixtures and review staged content before publication.

Pairing should persist through the host OS credential store where available; never silently fall back to plaintext. Handle unavailable or locked stores and revoked pairing explicitly. Verify TV identity and SSH host keys; do not disable verification to make a connection succeed. Do not send credentials to an MCP client or model. Screenshots sent to a remote model leave the user's machine.

Support macOS, Windows, and Linux without hard-coded local paths or a specific credential manager. Record which platforms and TV models were actually tested. Setup must not root a TV, enable SSH, install software, or change TV settings as a side effect.

## Verification

Follow the relevant unit in `plan.md`. Use fake devices and credential stores for repeatable failure and lifecycle tests, and label simulated evidence. For user-authorized real-TV checks, inspect the actual screen, account for the shared device, and verify cleanup. Do not equate a successful API response with a visible result.

Once the Rust project exists, run `cargo fmt --check`, `cargo clippy`, and `cargo test` for relevant changes. Exercise the compiled server with an MCP client for protocol changes. Rerun TLC when changing lifecycle rules or models. For performance work, compare matched end-to-end journeys and report failures and measurement limits; the 2–3× improvement is a target, not a fact.

Do not commit raw TLC logs, screenshots, or other private evidence. Run a secret scan when available and inspect the files and Git history that will be published. A clean scan is not proof that all sensitive information is absent.
