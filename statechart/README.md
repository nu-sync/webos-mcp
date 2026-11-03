# TV session statechart

This is a proposed high-level model, not an implementation or a claim about the existing Python helpers. MCP server lifetime and the TV session are distinct: the server starts disconnected, may connect and disconnect repeatedly, and cleans its owned connection resources on a graceful server stop. A screenshot is required before sending an action and after every action. An action timeout produces an uncertain outcome. The current model permits another action after a fresh observation only under the late-delivery assumption below; the MVP must follow the stricter recovery gate in [SPEC.md](../SPEC.md). A screenshot failure does not return an old image as a new one.

`resources = "owned"` abstracts task-owned tunnel and connections, including resources acquired during connection setup. Successful cleanup is modeled as a transition; the model does not prove OS cleanup or handle SIGKILL. The model assumes an action in flight will not take effect after its subsequent recovery observation; real transports must establish or replace this assumption before relying on a screenshot as permission to retry. A capture cannot prove the TV's full physical display or that an acknowledged action achieved its goal. Freshness is relative to model-controlled actions: this model does not represent a person using the physical remote, screen animation, stale network responses, duplicate commands, cancellation races, repeated button batches, or individual buttons. It checks safety and deadlock, not eventual completion under network failure; external failure and time are nondeterministic and no fairness is asserted.

To regenerate the chart, install the statechart skill and run its launcher from this repository with a new output folder. For example, set `STATECHART_SKILL_DIR` to your local skill directory and run:

```sh
sh "$STATECHART_SKILL_DIR/scripts/statechart" statechart/TvSession.tla \
  --config statechart/TvSession.cfg --view statechart/view.mjs \
  --out /tmp/webos-statechart-rerun
```

The generated report summarizes the November 3, 2026 run. Raw TLC logs can expose local paths and environment details and are not included. The chart omits only the terminal self-loop; it shows all other modeled transitions.
