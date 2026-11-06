"""Check that TLC detects persisting an address before TV authentication."""
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
launcher = Path(os.environ['STATECHART_SKILL_DIR']) / 'scripts/statechart'
source = (root / 'AddressSetup.tla').read_text()
old = 'candidate\' = (IF saved = "none" THEN "old" ELSE "new") /\\ UNCHANGED <<saved, exposed, trusted, lastCommitTrusted>>'
new = 'candidate\' = (IF saved = "none" THEN "old" ELSE "new") /\\ saved\' = (IF saved = "none" THEN "old" ELSE "new") /\\ lastCommitTrusted\' = FALSE /\\ UNCHANGED <<exposed, trusted>>'
if source.count(old) != 1:
    raise SystemExit('Fault injection did not match the current model')
source = source.replace('MODULE AddressSetup', 'MODULE AddressSetupFault', 1).replace(old, new, 1)
with tempfile.TemporaryDirectory(prefix='webos-address-fault-') as directory:
    path = Path(directory)
    spec = path / 'AddressSetupFault.tla'
    spec.write_text(source)
    result = subprocess.run(['sh', str(launcher), str(spec), '--config', str(root / 'AddressSetup.cfg'),
                             '--view', str(root / 'address-view.mjs'), '--out', str(path / 'out')],
                            capture_output=True, text=True, timeout=120)
    report = path / 'out' / 'report.md'
    content = report.read_text() if report.exists() else ''
    if result.returncode != 1 or 'CommitRequiresTrustedTV breaks' not in content:
        raise SystemExit('Fault was not detected as expected; inspect the model and rerun')
    print('Expected violation: an address commit without verified TV identity is rejected.')
