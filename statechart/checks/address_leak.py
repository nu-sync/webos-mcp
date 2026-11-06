"""Verify TLC catches an intentional public-address disclosure."""
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
launcher = Path(os.environ['STATECHART_SKILL_DIR']) / 'scripts/statechart'
source = (root / 'AddressSetup.tla').read_text()
old = 'Next == Enter \\/ Submit \\/ Verify \\/ Reject \\/ Commit \\/ StoreFailed \\/ StoreUnconfirmed \\/ Interrupt \\/ Forget \\/ Wait'
new = 'Leak == /\\ phase = "saved" /\\ exposed\' = TRUE /\\ UNCHANGED <<phase, saved, candidate, trusted, lastCommitTrusted>>\nNext == Enter \\/ Submit \\/ Verify \\/ Reject \\/ Commit \\/ StoreFailed \\/ StoreUnconfirmed \\/ Interrupt \\/ Forget \\/ Wait \\/ Leak'
if source.count(old) != 1:
    raise SystemExit('Fault injection did not match the current model')
source = source.replace('MODULE AddressSetup', 'MODULE AddressLeak', 1).replace(old, new, 1)
with tempfile.TemporaryDirectory(prefix='webos-address-fault-') as directory:
    path = Path(directory)
    spec = path / 'AddressLeak.tla'
    spec.write_text(source)
    result = subprocess.run(['sh', str(launcher), str(spec), '--config', str(root / 'AddressSetup.cfg'),
                             '--view', str(root / 'address-view.mjs'), '--out', str(path / 'out')],
                            capture_output=True, text=True, timeout=120)
    report = path / 'out' / 'report.md'
    content = report.read_text() if report.exists() else ''
    if result.returncode != 1 or 'AddressNotExposed breaks' not in content:
        raise SystemExit('Disclosure was not detected as expected')
    print('Expected violation: publishing a saved address breaks AddressNotExposed.')
