"""Build the real installer for approved VM acceptance; never run or publish it."""
import hashlib
import argparse
import json
from pathlib import Path
import subprocess
import tomllib
import os

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', required=True)
parser.add_argument('--notice-directory', required=True)
parser.add_argument('--build-receipt', required=True)
parser.add_argument('--iscc', required=True, help='Path to the reviewed Inno Setup compiler')
parser.add_argument('--notice-sha256', required=True, help='Reviewed SHA-256 of THIRD-PARTY-NOTICES.txt')
args = parser.parse_args()
output = (root / args.output).resolve()
if output == root / 'target' or not output.is_relative_to(root / 'target'):
    raise SystemExit('Output must be a fresh child of this project target directory.')
notice_dir = root / args.notice_directory
notices = notice_dir / 'THIRD-PARTY-NOTICES.txt'
report = json.loads((notice_dir / 'notices-report.json').read_text())
base_path = root / args.build_receipt
base_raw = base_path.read_bytes()
base = json.loads(base_raw)
def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

if (not report['complete_text_collection'] or report['missing']
        or report['cargo_lock_sha256'] != digest(root / 'Cargo.lock')
        or report['build_receipt_sha256'] != hashlib.sha256(base_raw).hexdigest()
        or base['cargo_toml_sha256'] != digest(root / 'Cargo.toml')
        or len(args.notice_sha256) != 64
        or any(c not in '0123456789abcdef' for c in args.notice_sha256)
        or digest(notices) != args.notice_sha256):
    raise SystemExit('Notice inputs are incomplete, changed or stale.')
output.mkdir()  # preserve every preceding build and receipt
common = ['--locked', '--offline', '--release', '--no-default-features',
          '--features', 'installer-tools', '--bins', '--target', 'x86_64-pc-windows-msvc',
          '--target-dir', 'target/xwin-hotkeys', '--jobs', '2']
cargo = ['cargo'] if os.name == 'nt' else ['cargo', 'xwin']
subprocess.run([*cargo, 'clippy', *common, '--', '-D', 'warnings'], cwd=root, timeout=1800, check=True)
build = subprocess.run([*cargo, 'build', *common, '--message-format=json'], cwd=root,
                       stdout=subprocess.PIPE, timeout=1800, check=True)
compiled = {item['package_id'] for line in build.stdout.splitlines()
            if (item := json.loads(line)).get('reason') == 'compiler-artifact'}
if not compiled or not compiled.issubset(set(base['compiled_package_ids'])):
    raise SystemExit('Actual installer binaries use dependencies outside the notice receipt.')
def windows(path):
    if os.name == 'nt':
        return str(path)
    return subprocess.check_output(['wslpath', '-w', str(path)], text=True).strip()

release = root / 'target/xwin-hotkeys/x86_64-pc-windows-msvc/release'
app, helper = release / 'AutoKeyboardLayot.exe', release / 'installer-package-helper.exe'
version = tomllib.loads((root / 'Cargo.toml').read_text())['package']['version']
command = [args.iscc,
           '/DAppVersion=' + version, '/DAppExecutable=' + windows(app),
           '/DPackageHelper=' + windows(helper), '/DBundleNotices=' + windows(notices),
           '/O' + windows(output), windows(root / 'installer/AutoKeyboardLayot.iss')]
subprocess.run(command, cwd=root, timeout=1200, check=True)
artifacts = list(output.glob('*.exe'))
if len(artifacts) != 1 or artifacts[0].name != f'AutoKeyboardLayot-{version}-setup-experimental.exe':
    raise SystemExit('Unexpected installer artifact.')
receipt = {'state': 'built_for_vm_acceptance_not_executed', 'installation_disabled': False,
           'notices_complete_text_collection': True, 'redistribution_approval': False,
           'published': False, 'installer_acceptance': False,
           'source_manifest_sha256': digest(root / 'Cargo.toml'), 'cargo_lock_sha256': digest(root / 'Cargo.lock'),
           'notice_sha256': digest(notices), 'compiled_package_ids': sorted(compiled),
           'artifacts': [{'file': str(path.relative_to(root)), 'bytes': path.stat().st_size, 'sha256': digest(path)}
                         for path in [app, helper, artifacts[0]]]}
with (output / 'build.json').open('x', encoding='utf-8') as stream:
    json.dump(receipt, stream, indent=2)
print('EXPERIMENTAL_INSTALLER_BUILT_FOR_VM_NOT_EXECUTED', flush=True)
