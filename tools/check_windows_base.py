"""Compile/check the modular Windows base; no execution, installation or upload."""
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
common = ['--release', '--all-targets', '--locked', '--offline', '--no-default-features',
          '--target', 'x86_64-pc-windows-msvc', '--target-dir', 'target/xwin-hotkeys', '--jobs', '2']
for stage in ('build', 'clippy'):
    command = ['cargo', 'xwin', stage, *common]
    if stage == 'clippy':
        command += ['--', '-D', 'warnings']
    print('WINDOWS_BASE_' + stage.upper() + '_START', flush=True)
    result = subprocess.run(command, cwd=root, timeout=1800)
    if result.returncode:
        sys.exit(result.returncode)
print('WINDOWS_BASE_BUILD_CHECKS_PASSED', flush=True)
