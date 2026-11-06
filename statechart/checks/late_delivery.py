"""Verify TLC detects an intentionally unsafe late-delivery recovery."""
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
launcher = Path(os.environ['STATECHART_SKILL_DIR']) / 'scripts/statechart'
source = (root / 'TvSession.tla').read_text()
old = 'UncertainObservationDone == /\\ phase = "observing_uncertain" /\\ phase\' = "uncertain" /\\ lastResult\' = "action_uncertain"'
new = 'UncertainObservationDone == /\\ phase = "observing_uncertain" /\\ lateApplied /\\ phase\' = "ready" /\\ lastResult\' = "fresh"'
if source.count(old) != 1:
    raise SystemExit('Fault injection did not match the current model')
source = source.replace('MODULE TvSession', 'MODULE TvSessionFault', 1).replace(old, new, 1)
with tempfile.TemporaryDirectory(prefix='webos-model-fault-') as directory:
    path = Path(directory)
    spec = path / 'TvSessionFault.tla'
    spec.write_text(source)
    result = subprocess.run(['sh', str(launcher), str(spec), '--config', str(root / 'TvSession.cfg'),
                             '--view', str(root / 'view.mjs'), '--out', str(path / 'out')],
                            capture_output=True, text=True, timeout=120)
    report = path / 'out' / 'report.md'
    content = report.read_text() if report.exists() else ''
    if result.returncode != 1 or 'ReadyHasFreshImage breaks' not in content or 'Delayed action arrives' not in content:
        raise SystemExit('Fault was not detected as expected; inspect the model and rerun')
    print('Expected violation: a delayed action followed by an unsafe observation can incorrectly allow Ready.')
