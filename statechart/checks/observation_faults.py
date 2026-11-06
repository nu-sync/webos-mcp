"""Require TLC to detect stale input, action replay, and false cleanup claims."""
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
launcher = Path(os.environ["STATECHART_SKILL_DIR"]) / "scripts/statechart"
source = (root / "Observation.tla").read_text()

faults = [
    (
        "stale_frame",
        "supplied = CurrentToken",
        "supplied[1] = generation",
        "ActionUsesCurrentObservation",
    ),
    (
        "stale_session",
        "supplied = CurrentToken",
        "supplied[2] = revision",
        "ActionUsesCurrentObservation",
    ),
    (
        "action_replay",
        '''CaptureFailed == /\\ phase = "capturing" /\\ phase' = "capture_failed"
                 /\\ fresh' = FALSE /\\ observation' = "failed"
                 /\\ UNCHANGED <<generation, revision, accepted, delivery, sends, cleanup, supplied>>''',
        '''CaptureFailed == /\\ phase = "capturing"
                 /\\ phase' = (IF delivery = "sent" THEN "sending" ELSE "capture_failed")
                 /\\ sends' = (IF delivery = "sent" THEN 2 ELSE sends)
                 /\\ fresh' = FALSE /\\ observation' = "failed"
                 /\\ UNCHANGED <<generation, revision, accepted, delivery, cleanup, supplied>>''',
        "AtMostOneSendPerCall",
    ),
    (
        "false_cleanup",
        '''CloseFailed == /\\ phase = "closing" /\\ phase' = "cleanup_unconfirmed"''',
        '''CloseFailed == /\\ phase = "closing" /\\ phase' = "disconnected"''',
        "CleanDisconnectIsConfirmed",
    ),
]

for name, original, replacement, invariant in faults:
    if source.count(original) != 1:
        raise SystemExit(f"Fault injection {name} no longer matches the model")
    mutated = source.replace(original, replacement, 1)
    with tempfile.TemporaryDirectory(prefix="webos-observation-fault-") as directory:
        path = Path(directory)
        spec = path / "Observation.tla"
        spec.write_text(mutated)
        view = path / "view.mjs"
        view.write_text((root / "observation-view.mjs").read_text().replace("return null;", "return labelsForStep[action] ?? action;"))
        result = subprocess.run(
            ["sh", str(launcher), str(spec), "--config", str(root / "Observation.cfg"),
             "--view", str(view), "--out", str(path / "out")],
            capture_output=True, text=True, timeout=120,
        )
        report = path / "out" / "report.md"
        content = report.read_text() if report.exists() else ""
        if result.returncode != 1 or f"{invariant} breaks" not in content:
            raise SystemExit(f"Expected invariant violation was not found for {name}")
        print(f"Expected violation detected: {name} -> {invariant}")
