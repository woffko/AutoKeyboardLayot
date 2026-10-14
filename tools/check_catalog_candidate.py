#!/usr/bin/env python3
"""Review a release catalog candidate against the catalog it renews.

    check_catalog_candidate.py PREVIOUS CANDIDATE [--days N] [--revision N] [--max-age-hours N] [--now UNIX]

PREVIOUS and CANDIDATE are catalog files, signed (.aklc) or unsigned (.json). A renewal is
the same package records under a new, higher revision and a new window, so the check passes
only when:

* both files are one well-formed catalog document of the expected shape and the same repository;
* the candidate's revision is PREVIOUS's plus one (or the value given with --revision);
* every package record is identical to PREVIOUS's, in the same order;
* the window is exactly --days long (default 21, 1 to 31), the same bounds as the signing tools;
* the window has started and started recently (--max-age-hours, default 3): the signing tool
  backdates it by one hour, so an older candidate means a delay since it was prepared, and the
  window then ends earlier than planned. Prepare the candidate again just before signing.

The check reads files only. It never signs, uploads or contacts anything. Exit status 0 means
the candidate passes; the summary names the file's SHA-256 for the record.
"""
import argparse
import hashlib
import json
import sys
import time
from pathlib import Path

MAX_BYTES = 1024 * 1024
CATALOG_FIELDS = {'expires_at', 'format', 'issued_at', 'packages', 'repository', 'revision'}
UNSIGNED_FIELDS = {'catalog', 'format'}
SIGNED_FIELDS = UNSIGNED_FIELDS | {'signature', 'signer'}
SECONDS_PER_DAY = 86400


class Rejected(Exception):
    pass


def no_repeated_fields(pairs):
    names = [name for name, _ in pairs]
    if len(names) != len(set(names)):
        raise Rejected(f'a field is written twice: {sorted({n for n in names if names.count(n) > 1})}')
    return dict(pairs)


def read_catalog(path):
    """Returns (catalog dict, signed?, sha256 hex) for a catalog file."""
    data = Path(path).read_bytes()
    if len(data) > MAX_BYTES:
        raise Rejected(f'{path}: larger than {MAX_BYTES} bytes')
    try:
        outer = json.loads(data.decode('utf-8'), object_pairs_hook=no_repeated_fields)
    except (UnicodeDecodeError, ValueError) as error:
        raise Rejected(f'{path}: not a JSON document ({error})') from error
    if not isinstance(outer, dict) or set(outer) not in (UNSIGNED_FIELDS, SIGNED_FIELDS):
        raise Rejected(f'{path}: unexpected fields {sorted(outer) if isinstance(outer, dict) else type(outer).__name__}')
    if outer['format'] != 1 or not isinstance(outer['catalog'], str):
        raise Rejected(f'{path}: not a format 1 catalog envelope')
    try:
        catalog = json.loads(outer['catalog'], object_pairs_hook=no_repeated_fields)
    except ValueError as error:
        raise Rejected(f'{path}: the catalog text is not JSON ({error})') from error
    if not isinstance(catalog, dict) or set(catalog) != CATALOG_FIELDS:
        raise Rejected(f'{path}: unexpected catalog fields')
    if catalog['format'] != 1:
        raise Rejected(f'{path}: not a format 1 catalog')
    for name in ('revision', 'issued_at', 'expires_at'):
        if type(catalog[name]) is not int or catalog[name] < 0:
            raise Rejected(f'{path}: {name} is not a whole number')
    if not isinstance(catalog['packages'], list) or not catalog['packages']:
        raise Rejected(f'{path}: no packages')
    return catalog, set(outer) == SIGNED_FIELDS, hashlib.sha256(data).hexdigest()


def check(previous_path, candidate_path, days=21, revision=None, max_age_hours=3.0, now=None):
    """Returns a list of summary lines; raises Rejected with the first problem found."""
    if not 1 <= days <= 31:
        raise Rejected('--days must be 1 to 31')
    now = int(time.time()) if now is None else now
    previous, previous_signed, _ = read_catalog(previous_path)
    candidate, candidate_signed, digest = read_catalog(candidate_path)
    if candidate['repository'] != previous['repository']:
        raise Rejected(f"repository differs: {candidate['repository']} instead of {previous['repository']}")
    expected_revision = previous['revision'] + 1 if revision is None else revision
    if candidate['revision'] != expected_revision or candidate['revision'] <= previous['revision']:
        raise Rejected(f"revision is {candidate['revision']}, expected {expected_revision} "
                       f"(clients that accepted {previous['revision']} refuse an equal or lower one)")
    if candidate['packages'] != previous['packages']:
        before = {p.get('package_id'): p for p in previous['packages']}
        after = {p.get('package_id'): p for p in candidate['packages']}
        changed = sorted(i for i in before.keys() | after.keys() if before.get(i) != after.get(i))
        detail = f'changed or missing records: {changed}' if changed else 'the order differs'
        raise Rejected(f'the package records differ from the previous catalog ({detail})')
    length = candidate['expires_at'] - candidate['issued_at']
    if length != days * SECONDS_PER_DAY:
        raise Rejected(f'the window is {length / SECONDS_PER_DAY:g} days, expected exactly {days}')
    if candidate['issued_at'] > now:
        raise Rejected('the window has not started yet (issued_at is in the future)')
    age_hours = (now - candidate['issued_at']) / 3600
    if age_hours > max_age_hours:
        raise Rejected(f'the window started {age_hours:.1f} hours ago (limit {max_age_hours:g}); '
                       'prepare the candidate again just before signing')
    remaining_days = (candidate['expires_at'] - now) / SECONDS_PER_DAY
    return [
        f"candidate: {'signed' if candidate_signed else 'unsigned'}, sha256 {digest}",
        f"previous:  {'signed' if previous_signed else 'unsigned'}, revision {previous['revision']}",
        f"revision {candidate['revision']}; {len(candidate['packages'])} package records, identical to the previous catalog",
        f"window {days} days, started {age_hours:.1f} hours ago, {remaining_days:.1f} days left",
        f"issued_at {candidate['issued_at']} ({time.strftime('%Y-%m-%d %H:%M:%S', time.gmtime(candidate['issued_at']))} UTC)",
        f"expires_at {candidate['expires_at']} ({time.strftime('%Y-%m-%d %H:%M:%S', time.gmtime(candidate['expires_at']))} UTC)",
    ]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('previous')
    parser.add_argument('candidate')
    parser.add_argument('--days', type=int, default=21)
    parser.add_argument('--revision', type=int)
    parser.add_argument('--max-age-hours', type=float, default=3.0)
    parser.add_argument('--now', type=int, help='Unix time to judge by (for tests and reviews of old candidates)')
    arguments = parser.parse_args(argv)
    try:
        lines = check(arguments.previous, arguments.candidate, arguments.days, arguments.revision,
                      arguments.max_age_hours, arguments.now)
    except (Rejected, OSError) as error:
        print(f'REJECTED: {error}', file=sys.stderr)
        return 1
    print('\n'.join(lines))
    print('CANDIDATE_OK')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
