"""Build exact base test executables and record their hashes; never run them."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
output = Path(sys.argv[1])
if output.exists():
    raise SystemExit('Use a new artifact manifest path.')
sources = {str(root / 'src/lib.rs'): 'core', str(root / 'src/main.rs'): 'adapter',
           str(root / 'tests/english_base_build.rs'): 'base_integration'}
command = ['cargo', 'xwin', 'test', '--release', '--locked', '--offline', '--no-default-features',
           '--target', 'x86_64-pc-windows-msvc', '--target-dir', 'target/xwin-hotkeys', '--jobs', '2',
           '--no-run', '--message-format=json']
result = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, timeout=1200, check=True)
artifacts = {}
compiled_packages = set()
for line in result.stdout.splitlines():
    item = json.loads(line)
    if item.get('reason') == 'compiler-artifact':
        compiled_packages.add(item['package_id'])
    if item.get('reason') != 'compiler-artifact' or not item['profile']['test'] or not item.get('executable'):
        continue
    tag = sources.get(item['target']['src_path'])
    if tag:
        executable = Path(item['executable'])
        expected = root / 'target/xwin-hotkeys/x86_64-pc-windows-msvc/release/deps'
        if executable.parent != expected or tag in artifacts:
            raise SystemExit('Unexpected test artifact location or duplicate.')
        artifacts[tag] = {'file': executable.name, 'sha256': hashlib.sha256(executable.read_bytes()).hexdigest()}
if set(artifacts) != set(sources.values()):
    raise SystemExit('Missing expected test executable.')
with output.open('x', encoding='utf-8') as stream:
    json.dump({'variant': 'base', 'artifacts': artifacts,
               'cargo_lock_sha256': hashlib.sha256((root / 'Cargo.lock').read_bytes()).hexdigest(),
               'cargo_toml_sha256': hashlib.sha256((root / 'Cargo.toml').read_bytes()).hexdigest(),
               'compiled_package_ids': sorted(compiled_packages)}, stream, indent=2)
print(json.dumps({'artifacts': artifacts, 'compiled_packages': len(compiled_packages),
                 'skia': sorted(item for item in compiled_packages if '#skia-' in item)}))
