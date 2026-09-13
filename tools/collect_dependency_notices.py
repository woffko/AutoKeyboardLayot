"""Collect local license texts; do not infer or select a redistribution license."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import urllib.parse


def license_paths(package):
    root = Path(package['manifest_path']).parent.resolve()
    paths = set()
    for path in root.iterdir():
        if not path.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE')):
            continue
        if path.is_symlink():
            raise ValueError('linked license source')
        if path.is_file():
            paths.add(path)
        elif path.is_dir():
            paths.update(item for item in path.rglob('*') if item.is_file())
    if len(paths) > 1000:
        raise ValueError('too many license texts')
    if package.get('license_file'):
        paths.add(root / package['license_file'])
    result = []
    for path in sorted(paths):
        resolved = path.resolve(strict=True)
        if not resolved.is_relative_to(root) or path.is_symlink():
            raise ValueError('license path leaves its package')
        with resolved.open('rb') as stream:
            raw = stream.read(1024 * 1024 + 1)
        if len(raw) > 1024 * 1024 or b'\0' in raw:
            raise ValueError('invalid license text')
        result.append((str(resolved.relative_to(root)), raw.decode('utf-8-sig'),
                       hashlib.sha256(raw).hexdigest()))
    return result


def curated_texts(package):
    # Audited license-only child commit; not a generic provenance override.
    if (package['name'], package['version']) != ('simd_helpers', '0.1.0'):
        return []
    vcs = json.loads((Path(package['manifest_path']).parent / '.cargo_vcs_info.json').read_text(encoding='utf-8'))
    if (package.get('repository') != 'https://github.com/lu-zero/simd_helpers'
            or package.get('license') != 'MIT'
            or vcs['git']['sha1'] != 'ca1a2f84aa386d758e98f8a609d990263932fb85'
            or vcs['git'].get('dirty', False)):
        raise ValueError('curated simd_helpers provenance mismatch')
    path = Path(__file__).resolve().parents[1] / 'data/dependency-notices/simd_helpers-0.1.0/LICENSE'
    if path.is_symlink():
        raise ValueError('linked curated notice')
    raw = path.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != 'd69f24ad84ec2ade64c0b68bdb31b41170e997b158370342056918329cc9af1e':
        raise ValueError('curated notice hash mismatch')
    source = 'https://raw.githubusercontent.com/lu-zero/simd_helpers/82040194cd05affb060bf94d6f19f82a771d07fb/LICENSE'
    return [(source, raw.decode('utf-8'), digest)]


def upstream_texts(package, entry, directory):
    root = Path(package['manifest_path']).parent
    vcs = json.loads((root / '.cargo_vcs_info.json').read_text(encoding='utf-8'))
    revision = vcs['git']['sha1']
    repository = (package.get('repository') or '').removeprefix('https://github.com/').rstrip('/').removesuffix('.git')
    if package['name'] == 'zune-inflate' and package['version'] == '0.2.54' and package.get('homepage') == 'https://github.com/etemesi254/zune-image/tree/main/zune-inflate':
        repository = 'etemesi254/zune-image'
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repository):
        raise ValueError('invalid upstream repository')
    if entry['revision'] != revision or entry['repository'] != repository:
        raise ValueError('upstream provenance mismatch')
    dirty = bool(vcs['git'].get('dirty'))
    if entry['dirty_source'] != dirty:
        raise ValueError('upstream dirty flag mismatch')
    result = []
    for item in entry['files']:
        path = directory / item['path']
        resolved = path.resolve(strict=True)
        if path.is_symlink() or not resolved.is_relative_to(directory.resolve()):
            raise ValueError('upstream license path escape')
        with resolved.open('rb') as stream:
            raw = stream.read(1024 * 1024 + 1)
        if len(raw) > 1024 * 1024 or b'\0' in raw or hashlib.sha256(raw).hexdigest() != item['sha256']:
            raise ValueError('upstream text mismatch')
        prefix = f'https://raw.githubusercontent.com/{repository}/{revision}/'
        parsed_url = urllib.parse.urlsplit(item['source_url'])
        if not item['source_url'].startswith(prefix) or parsed_url.query or parsed_url.fragment:
            raise ValueError('upstream URL mismatch')
        result.append((item['source_url'], raw.decode('utf-8-sig'), item['sha256']))
    return result, dirty


def packages_from_receipt(metadata, receipt, lock_hash, manifest_hash):
    if receipt['variant'] != 'base' or receipt['cargo_lock_sha256'] != lock_hash or receipt['cargo_toml_sha256'] != manifest_hash:
        raise ValueError('build receipt does not match this base configuration')
    included = set(receipt['compiled_package_ids'])
    available = {package['id'] for package in metadata['packages']}
    if metadata['resolve']['root'] not in included or not included.issubset(available):
        raise ValueError('invalid compiled package set')
    return [package for package in metadata['packages'] if package['id'] in included]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path, help='new output directory')
    parser.add_argument('--upstream', type=Path, help='version-pinned upstream-notices.json')
    parser.add_argument('--build-receipt', type=Path, help='actual Cargo compiler-artifact receipt for the base test build')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    metadata_command = [
        'cargo', 'metadata', '--locked', '--offline', '--no-default-features',
        '--format-version', '1',
    ]
    # The actual build also uses host-platform proc-macro/build dependencies.
    # Resolve their metadata too, then select by compiler-artifact package IDs.
    if not args.build_receipt:
        metadata_command += ['--filter-platform', 'x86_64-pc-windows-msvc']
    metadata = json.loads(subprocess.check_output(metadata_command, cwd=root, timeout=30))
    lock_hash = hashlib.sha256((root / 'Cargo.lock').read_bytes()).hexdigest()
    packages = metadata['packages']
    receipt_hash = None
    if args.build_receipt:
        raw_receipt = args.build_receipt.read_bytes()
        packages = packages_from_receipt(metadata, json.loads(raw_receipt), lock_hash,
                                        hashlib.sha256((root / 'Cargo.toml').read_bytes()).hexdigest())
        receipt_hash = hashlib.sha256(raw_receipt).hexdigest()
    upstream = {}
    if args.upstream:
        supplement = json.loads(args.upstream.read_text(encoding='utf-8'))
        if supplement['cargo_lock_sha256'] != lock_hash:
            raise ValueError('upstream Cargo.lock mismatch')
        for entry in supplement['entries']:
            if entry['package'] in upstream:
                raise ValueError('duplicate upstream package')
            upstream[entry['package']] = entry
    sections = ['AutoKeyboardLayot: collected third-party notices\n',
        'This collection includes the resolved Windows base dependency graph,\n'
        'including build/test dependencies. License alternatives are reproduced\n'
        'as metadata, not selected. This is not redistribution approval.\n']
    records, missing = [], []
    for package in sorted(packages, key=lambda p: (p['name'], p['version'])):
        if Path(package['manifest_path']).resolve() == root / 'Cargo.toml':
            continue
        identity = package['name'] + ' ' + package['version']
        try:
            texts = license_paths(package)
            if not texts:
                texts = curated_texts(package)
            if not texts and identity in upstream:
                texts, dirty = upstream_texts(package, upstream[identity], args.upstream.parent)
                if dirty:
                    missing.append({'package': identity, 'reason': 'upstream text collected but source is dirty; review required'})
        except (OSError, UnicodeError, ValueError) as error:
            missing.append({'package': identity, 'reason': type(error).__name__})
            continue
        if not texts:
            missing.append({'package': identity, 'reason': 'no bundled license text'})
        sections.append('\n=== ' + identity + ' ===\nDeclared license: ' +
                        (package.get('license') or '(not declared)') + '\n')
        files = []
        for name, text, digest in texts:
            sections.append('\n--- ' + name + ' ---\n' + text + '\n')
            files.append({'file': name, 'sha256': digest})
        records.append({'package': identity, 'license': package.get('license'), 'files': files})
    dictionary = root / 'data/language-packs/en-US/LICENSE.words.md'
    sections.append('\n=== Bundled English dictionary ===\n' + dictionary.read_text(encoding='utf-8'))
    args.output.mkdir(parents=False, exist_ok=False)
    filename = 'THIRD-PARTY-NOTICES.incomplete.txt' if missing else 'THIRD-PARTY-NOTICES.txt'
    (args.output / filename).write_text('\n'.join(sections), encoding='utf-8')
    report = {'complete_text_collection': not missing, 'redistribution_approval': False,
              'cargo_lock_sha256': lock_hash,
              'scope': 'compiled base test graph (host and target)' if receipt_hash else 'Windows metadata projection',
              'build_receipt_sha256': receipt_hash,
              'packages': records, 'missing': missing}
    (args.output / 'notices-report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps({'packages': len(records), 'missing': missing, 'output': str(args.output / filename)}))
    return 2 if missing else 0


if __name__ == '__main__':
    raise SystemExit(main())
