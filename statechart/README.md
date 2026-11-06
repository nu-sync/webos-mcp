# TV lifecycle models

These are proposed models, not proofs of an implementation. Re-run the affected models after changing their contracts; keep raw TLC output outside the repository because it can reveal local paths and machine details.

## TV session

`TvSession.tla` models a server that starts disconnected, connects to a TV, requires a fresh observation before an action, and closes its own resources. A timed-out or interrupted in-flight action remains unresolved across disconnect and reconnect. A screenshot while unresolved permits inspection but cannot make another action safe. An action sent late can occur during that uncertainty. Only `DeliverySettled`, an abstract event backed by reliable external evidence, clears the gate; a fresh screenshot is then required.

A failed connection keepalive moves a fresh or observation-needed session to a stale state where actions are blocked until reconnection. A preflight check runs after a fresh observation and before any action is sent. A failed or cancelled check requires another observation and creates no pending-action marker; an error after the send begins remains unresolved. The preflight itself cannot prove future delivery or settle an earlier uncertain action.

No mechanism to provide that evidence has been verified. Do not implement `DeliverySettled` as an unconditional timeout or assume socket closure drains the TV queue. If no reliable proof is available, the session remains blocked. The model abstracts resources and assumes cleanup transitions succeed; it does not prove OS cleanup. The button transport returns after a local WebSocket send, not a TV acknowledgement; the model's `ActionAcknowledged` transition therefore abstracts an assumption about delivery that has not been established. A fresh image shows the captured screen, not whether a previously queued button could arrive later.

The model now includes a durable action marker and a crash transition. It assumes that marker is committed before any action can be sent, that its update is atomic and survives process death, and that acknowledgement justifies clearing it. These assumptions require implementation and transport evidence. The model does not represent shared physical-remote input, model-facing image content, actual time, retries, multiple server processes, or a real transport. No fairness or eventual completion is asserted. Internal keepalive lock arbitration is covered by implementation tests, not this model; a waiting user request checks health and observation identity after the keepalive releases the device.

## Observation and recovery

`Observation.tla` models the combined action and capture loop, request tokens from current and previous observations and sessions, capture failure, recovery, and confirmed cleanup while retaining the unresolved-action guard. A fresh image is not proof of an observed change or a settled UI. The model abstracts channel failure, authenticated reconnect, cancellation, cleanup, the physical screen, timing, other actors, and the actual transport; it does not prove the Rust implementation. It models recovery in general, while the current implementation permits capture-only recovery solely after a channel-open failure and confirmed SSH closure.

```sh
sh "$STATECHART_SKILL_DIR/scripts/statechart" statechart/Observation.tla \
  --config statechart/Observation.cfg --view statechart/observation-view.mjs \
  --out /tmp/webos-observation-check
```

TLC checked 432 states and 859 steps, the listed invariants, and deadlocks. The chart draws 11 states and 23 arrows; the [report](observation-report.md) records omitted same-state and repeated disconnect transitions. Four negative controls remove token guards, replay an action after capture failure, or falsely confirm cleanup. Run `python3 statechart/checks/observation_faults.py` with the same skill environment to verify that TLC catches each mutation. Rust fault-injection tests exercise the corresponding paths against `SessionOwner`.

## Onboarding

`Onboarding.tla` models setup, identity verification, saved-credential reauthorization, pairing, secure credential persistence, capability checking, interruption, revocation, and local removal. A rejected saved authorization leads to repair. An interrupted setup can restart; a successfully saved credential survives interruption. A supported screenshot capability is required before visual control can be reported ready. An unavailable secure store does not fall back to plaintext.

The model abstracts the credential store's atomicity, host/certificate verification, capability detection, and cleanup as reliable transitions. It does not model session-only credentials, multiple devices, credential-store crashes during an atomic save, actual key material, OS prompts, or concurrent clients. Local removal is not TV-side revocation. Extend the model before adding behavior that depends on those cases.

## Reproduce

Set `STATECHART_SKILL_DIR` to the installed statechart skill directory, then run from the repository root. Each output path must be new:

```sh
sh "$STATECHART_SKILL_DIR/scripts/statechart" statechart/TvSession.tla \
  --config statechart/TvSession.cfg --view statechart/view.mjs \
  --out /tmp/webos-session-check
sh "$STATECHART_SKILL_DIR/scripts/statechart" statechart/Onboarding.tla \
  --config statechart/Onboarding.cfg --view statechart/onboarding-view.mjs \
  --out /tmp/webos-onboarding-check
python3 statechart/checks/late_delivery.py
python3 statechart/checks/preflight_pending.py
```

The negative controls intentionally permit readiness after an unresolved late delivery and mark an action pending before the preflight; each succeeds only when TLC finds the expected invariant violation. The session chart hides repeated crash, shutdown, and disconnect transitions from some intermediate states; those transitions remain in the checked model. `chart.mmd` and `onboarding-chart.mmd` show the full projected graphs; transitions that do not change a displayed state are omitted by the chart generator. The reports summarize the checked models.

## Private address cache

`AddressSetup.tla` abstracts entering, verifying the TV, securely storing, reusing, replacing, and removing an address. Verification and pairing precede committing the new address and credential as one record. A failure or interruption before commit leaves the old saved value intact. When a write succeeds but its readback cannot be verified, the store may contain either the old or the new record; the model reports an unavailable state instead of claiming either outcome. `exposed` models an observable leak and no normal transition sets it. The model assumes that the secure store commits atomically and that its private contents are not exposed. TLC cannot prove either the operating system credential store's confidentiality, crash behavior, or the Rust program's output handling. The implementation must test those boundaries and report platform limitations.

```sh
sh "$STATECHART_SKILL_DIR/scripts/statechart" statechart/AddressSetup.tla \
  --config statechart/AddressSetup.cfg --view statechart/address-view.mjs \
  --out /tmp/webos-address-check
python3 statechart/checks/address_leak.py
python3 statechart/checks/address_before_trust.py
```

The negative controls inject a public disclosure and an address commit before identity verification. TLC must report `AddressNotExposed` and `CommitRequiresTrustedTV` respectively. The chart omits six transitions that leave the displayed state unchanged.

## Credential-store choice

`CredentialChoice.tla` checks that an unencrypted local credential file is selected explicitly and saved only after TV verification. It abstracts filesystem permissions, atomic replacement, backup access, and migration from the operating-system store. It does not establish confidentiality of plaintext files or verify Windows ACLs. The chart omits six transitions that leave the displayed state unchanged.

```sh
sh "$STATECHART_SKILL_DIR/scripts/statechart" statechart/CredentialChoice.tla \
  --config statechart/CredentialChoice.cfg --view statechart/credential-choice-view.mjs \
  --out /tmp/webos-credential-choice-check
```
