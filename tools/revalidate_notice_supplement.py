"""Reuse cached public notices only after revalidating current source and hashes."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
from collect_dependency_notices import upstream_texts


def revalidate(packages, supplement, required, directory):
    current = {}
    for package in packages:
        identity = package['name'] + ' ' + package['version']
        if identity not in required:
            continue
        if identity in current:
            raise ValueError('ambiguous current source identity')
        current[identity] = package
    cached = {}
    for entry in supplement['entries']:
        if entry['package'] in cached:
            raise ValueError('duplicate cached identity')
        cached[entry['package']] = entry
    if not required.issubset(current) or not required.issubset(cached):
        raise ValueError('missing current or cached identity')
    result = []
    for identity in sorted(required):
        entry = cached[identity]
        texts, dirty = upstream_texts(current[identity], entry, directory)
        if dirty or not texts:
            raise ValueError('dirty source or empty notice set')
        result.append(entry)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('cached', type=Path)
    parser.add_argument('current_report', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    if args.output.exists() or args.output.parent.resolve() != args.cached.parent.resolve():
        raise ValueError('new manifest must share the cached notice directory')
    root = Path(__file__).resolve().parents[1]
    lock_hash = hashlib.sha256((root / 'Cargo.lock').read_bytes()).hexdigest()
    report = json.loads(args.current_report.read_text(encoding='utf-8'))
    if report['cargo_lock_sha256'] != lock_hash:
        raise ValueError('current report is stale')
    required = {entry['package'] for entry in report['missing']}
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--offline',
                         '--no-default-features', '--format-version', '1'], cwd=root, timeout=30))
    raw = args.cached.read_bytes()
    entries = revalidate(metadata['packages'], json.loads(raw), required, args.cached.parent)
    result = {'cargo_lock_sha256': lock_hash, 'redistribution_approval': False,
              'revalidated_from_sha256': hashlib.sha256(raw).hexdigest(),
              'entries': entries, 'failures': []}
    with args.output.open('x', encoding='utf-8') as stream:
        json.dump(result, stream, indent=2)
    print(json.dumps({'revalidated_notice_groups': len(entries), 'network_used': False}))


if __name__ == '__main__':
    main()
