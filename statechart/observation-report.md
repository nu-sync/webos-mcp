# Observation

TLC reached 432 states and 859 steps. The chart draws 11 states and 23 arrows.
The chart does not draw 180 steps that stay in one chart state.
The view hides 90 steps, so the chart does not draw them.

## Rules that hold
TypeOK, ActionUsesCurrentObservation, AtMostOneSendPerCall, RecoveryPreservesUncertainty, ReadyRequiresFreshFrame, FailureInvalidatesObservation, CleanDisconnectIsConfirmed, CleanupBlocksActions, NoDeadlock

## Reader's rules

- `ActionUsesCurrentObservation`: an action can use only the current frame from the current session generation.
- `AtMostOneSendPerCall`: a failed result capture or capture recovery cannot send the action a second time.
- `RecoveryPreservesUncertainty`: observing, recovering, or reconnecting cannot resolve uncertain delivery.
- `ReadyRequiresFreshFrame`: readiness requires a new frame and no unresolved delivery.
- `FailureInvalidatesObservation`: capture failure revokes the observation used for future actions.
- `CleanDisconnectIsConfirmed`: disconnected is reported only after cleanup is confirmed.
- `CleanupBlocksActions`: pending or unconfirmed cleanup cannot permit another action.

## Scope and assumptions

The finite model explores one combined action-and-observation call, at most three
frame revisions in each of two session generations, and a possible reconnect.
A later independent action call is outside this bound. One model send denotes one requested action or bounded button batch; a batch may contain up to five paced physical button messages. The invariant forbids replaying that action or batch, not its explicitly requested constituent presses. The request independently supplies either the current token, an older frame
revision, or a token from the previous session. The send transition compares
that supplied identity with the current generation and frame revision before
accepting it. Tokens stand for unique session/frame identities; this model does not prove the implementation's token
allocation or storage. A transport send completion is recorded as `sent`, never
as proof that the TV applied the action. Both a fresh screenshot and a stable
image can precede a visible TV update; no UI-settled claim is modeled.

Recovery abstracts reauthentication and permitted read-only retries. It does
not prove that a failed transport or remote command was cleaned up, that an
image contains all physical screen layers, or that a device will eventually
respond. No fairness or eventual completion is asserted. Rooted SSH, OS storage,
private credentials, multiple processes, actual time, physical remote input,
crash durability, and the older TV-session model's implementation assumptions
remain outside this model.

The chart hides disconnect transitions from `needs_view`, `capturing`,
`recovering`, `checking_token`, and `sending`; they remain checked. It also omits transitions that
leave the displayed state unchanged, such as remaining disconnected. Stale-token submission and rejection are
explicit visible transitions. No hidden self-transition sends an action.

## Reproduce

From the project root, set `STATECHART_SKILL_DIR` to the installed skill folder.
Use a new output directory for each run; keep raw output outside the repository.

```sh
sh "$STATECHART_SKILL_DIR/scripts/statechart" statechart/Observation.tla \
  --config statechart/Observation.cfg --view statechart/observation-view.mjs \
  --out /tmp/webos-observation-recheck
python3 statechart/checks/observation_faults.py
```

The negative controls remove the frame-revision or session-generation part
of the send guard, replay an action after its result capture fails, and report
disconnected despite failed cleanup. They are mutations of the model,
not claims that these faults were observed in the implementation. The Rust
failure-injection tests must exercise the corresponding paths independently.
