"""Fetch public upstream license texts at revisions recorded by Cargo packages."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import urllib.error
import urllib.parse
import urllib.request


class PublicGitHubRedirects(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        parsed = urllib.parse.urlsplit(newurl)
        if parsed.scheme != 'https' or parsed.hostname not in ('api.github.com', 'raw.githubusercontent.com') or parsed.username:
            raise ValueError('unexpected redirect')
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def read_url(url, limit):
    request = urllib.request.Request(url, headers={'User-Agent': 'AutoKeyboardLayot-license-collector'})
    with urllib.request.build_opener(PublicGitHubRedirects()).open(request, timeout=20) as response:
        raw = response.read(limit + 1)
    if len(raw) > limit:
        raise ValueError('response too large')
    return raw


def safe_path(value):
    if not isinstance(value, str) or len(value) > 512 or '\\' in value or '\0' in value:
        raise ValueError('invalid path')
    path = PurePosixPath(value)
    if path.is_absolute() or not value or any(part in ('', '.', '..') for part in value.split('/')):
        raise ValueError('invalid path')
    return path


def entries_at(repository, revision, path=''):
    suffix = '/' + urllib.parse.quote(path, safe='/') if path else ''
    url = f'https://api.github.com/repos/{repository}/contents{suffix}?ref={revision}'
    entries = json.loads(read_url(url, 2 * 1024 * 1024))
    if not isinstance(entries, list) or len(entries) > 1000:
        raise ValueError('unexpected directory listing')
    return entries


def collect(repository, revision, output):
    files = []
    for entry in entries_at(repository, revision):
        if not entry['name'].upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE')):
            continue
        path = safe_path(entry['path'])
        if entry['type'] == 'file':
            files.append(path)
        elif entry['type'] == 'dir':
            for child in entries_at(repository, revision, str(path)):
                if child['type'] == 'file' and PurePosixPath(child['path']).suffix.lower() in ('', '.txt', '.md', '.rst'):
                    files.append(safe_path(child['path']))
    if len(files) > 100:
        raise ValueError('too many license files')
    records = []
    for path in sorted(set(files)):
        url = f'https://raw.githubusercontent.com/{repository}/{revision}/{urllib.parse.quote(str(path), safe="/")}'
        raw = read_url(url, 1024 * 1024)
        raw.decode('utf-8-sig')
        if b'\0' in raw:
            raise ValueError('not license text')
        relative = Path(repository.replace('/', '_')) / revision / Path(str(path))
        target = output / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open('xb') as stream:
            stream.write(raw)
        records.append({'path': str(relative), 'source_url': url, 'sha256': hashlib.sha256(raw).hexdigest()})
    return records


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('report', type=Path)
    parser.add_argument('output', type=Path, help='new directory; never reused')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    report = json.loads(args.report.read_text(encoding='utf-8'))
    lock_hash = hashlib.sha256((root / 'Cargo.lock').read_bytes()).hexdigest()
    if report['cargo_lock_sha256'] != lock_hash:
        raise ValueError('Cargo.lock changed since collection')
    missing = {entry['package'] for entry in report['missing']}
    metadata = json.loads(subprocess.check_output([
        'cargo', 'metadata', '--locked', '--offline', '--no-default-features',
        '--format-version', '1',
    ], cwd=root, timeout=30))
    plan = []
    for package in metadata['packages']:
        identity = package['name'] + ' ' + package['version']
        if identity not in missing:
            continue
        repository = package.get('repository') or ''
        if identity == 'zune-inflate 0.2.54' and package.get('homepage') == 'https://github.com/etemesi254/zune-image/tree/main/zune-inflate':
            repository = 'https://github.com/etemesi254/zune-image'
        match = re.fullmatch(r'https://github.com/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/?', repository)
        if not match:
            raise ValueError('repository not allowlisted for ' + identity)
        vcs = json.loads((Path(package['manifest_path']).parent / '.cargo_vcs_info.json').read_text(encoding='utf-8'))
        revision = vcs['git']['sha1']
        if not re.fullmatch(r'[0-9a-f]{40}', revision):
            raise ValueError('invalid recorded revision')
        plan.append((identity, match[1].removesuffix('.git'), revision, bool(vcs['git'].get('dirty'))))
    if {item[0] for item in plan} != missing:
        raise ValueError('missing package is not in current metadata')
    args.output.mkdir(exist_ok=False)
    cache, records, failures = {}, [], []
    for identity, repository, revision, dirty in plan:
        key = (repository, revision)
        try:
            if key not in cache:
                cache[key] = collect(repository, revision, args.output)
            files = cache[key]
            if not files:
                raise ValueError('no root license texts')
            records.append({'package': identity, 'repository': repository, 'revision': revision,
                            'dirty_source': dirty, 'files': files})
            print(identity + ': collected; dirty_source=' + str(dirty), flush=True)
        except (OSError, ValueError, KeyError, urllib.error.URLError) as error:
            # A partially fetched group is not retried for another crate from
            # the same commit; retain its files and report the incomplete group.
            cache[key] = []
            failures.append({'package': identity, 'reason': type(error).__name__})
            print(identity + ': failed ' + type(error).__name__, flush=True)
    result = {'cargo_lock_sha256': lock_hash, 'redistribution_approval': False,
              'entries': records, 'failures': failures}
    (args.output / 'upstream-notices.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    print('UPSTREAM_NOTICE_COLLECTION_FINISHED', flush=True)
    return 2 if failures else 0


if __name__ == '__main__':
    raise SystemExit(main())
