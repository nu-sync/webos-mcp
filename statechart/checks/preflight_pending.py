"""Verify TLC detects marking an action pending before the liveness check."""
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
launcher = Path(os.environ['STATECHART_SKILL_DIR']) / 'scripts/statechart'
source = (root / 'TvSession.tla').read_text()
old = 'CheckConnection == /\\ phase = "ready" /\\ ~unresolved /\\ phase\' = "checking" /\\ lastResult\' = "none" /\\ UNCHANGED <<resources, unresolved, lateApplied, durable>>'
new = 'CheckConnection == /\\ phase = "ready" /\\ ~unresolved /\\ phase\' = "checking" /\\ lastResult\' = "none" /\\ durable\' = TRUE /\\ UNCHANGED <<resources, unresolved, lateApplied>>'
if source.count(old) != 1:
    raise SystemExit('Fault injection did not match the current model')
source = source.replace('MODULE TvSession', 'MODULE PreflightFault', 1).replace(old, new, 1)
with tempfile.TemporaryDirectory(prefix='webos-preflight-fault-') as directory:
    path = Path(directory)
    spec = path / 'PreflightFault.tla'
    spec.write_text(source)
    view = path / 'view.mjs'
    view.write_text('export const state = v => v.phase;\nexport const step = action => action;\n')
    result = subprocess.run(['sh', str(launcher), str(spec), '--config', str(root / 'TvSession.cfg'),
                             '--view', str(view), '--out', str(path / 'out')],
                            capture_output=True, text=True, timeout=120)
    report = path / 'out' / 'report.md'
    content = report.read_text() if report.exists() else ''
    if result.returncode != 1 or 'NoPendingMarkerBeforeSend breaks' not in content:
        raise SystemExit('Fault was not detected as expected')
    print('Expected violation: a failed preflight must not leave a pending action marker.')
