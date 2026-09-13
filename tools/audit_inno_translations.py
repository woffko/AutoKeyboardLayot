"""Fetch six upstream translation candidates and audit them; never install them."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import urllib.parse
from fetch_dependency_notices import read_url

LANGUAGES = {
    'zh-CN': ('Files/Languages', 'ChineseSimplified'),
    'id': ('Files/Languages/Unofficial', 'Indonesian'),
    'ur': ('Files/Languages/Unofficial', 'Urdu'),
    'bn': ('Files/Languages/Unofficial', 'Bengali'),
    'et': ('Files/Languages/Unofficial', 'Estonian'),
    'hi': ('Files/Languages/Unofficial', 'Hindi'),
}


def decode(raw):
    if raw.startswith((b'\xff\xfe', b'\xfe\xff')):
        return raw.decode('utf-16'), 'utf-16'
    try:
        return raw.decode('utf-8-sig'), 'utf-8'
    except UnicodeDecodeError:
        match = re.search(rb'^LanguageCodePage\s*=\s*(\d+)\s*$', raw, re.MULTILINE)
        if not match or match[1] == b'0':
            raise ValueError('translation encoding is not declared')
        encoding = 'cp' + match[1].decode('ascii')
        return raw.decode(encoding), encoding


def parse(text):
    section, sections = None, {}
    for number, raw_line in enumerate(text.splitlines(), 1):
        line = raw_line.strip()
        if not line or line.startswith(';'):
            continue
        if line.startswith('[') and line.endswith(']'):
            section = line[1:-1].lower()
            if section not in ('langoptions', 'messages', 'custommessages'):
                raise ValueError(f'non-message section at line {number}')
            sections.setdefault(section, {})
            continue
        if section is None or '=' not in line:
            raise ValueError(f'invalid message line {number}')
        key, value = line.split('=', 1)
        key = key.strip()
        if not re.fullmatch(r'[A-Za-z][A-Za-z0-9]*', key) or key in sections[section]:
            raise ValueError(f'invalid or duplicate key at line {number}')
        sections[section][key] = value
    return sections


def parameters(text):
    tokens = re.findall(r'%%|%[1-9]|\[(?:name(?:/ver)?|mb|gb)\]', text)
    return Counter(token for token in tokens if token != '%%')


def compare(reference, candidate):
    expected, actual = reference.get('messages', {}), candidate.get('messages', {})
    required = {key for key, value in expected.items() if value}
    missing = sorted(key for key in required if not actual.get(key))
    mismatches = sorted(key for key in required & actual.keys()
                        if actual[key] and parameters(expected[key]).keys() != parameters(actual[key]).keys())
    return {'missing_messages': missing, 'parameter_mismatches': mismatches,
            'extra_messages': sorted(actual.keys() - expected.keys()),
            'required_messages': len(required), 'provided_messages': len(actual)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reference', type=Path, help='installed compiler Default.isl')
    parser.add_argument('output', type=Path, help='new candidate directory')
    args = parser.parse_args()
    reference_raw = args.reference.read_bytes()
    reference = parse(decode(reference_raw)[0])
    commit = json.loads(read_url('https://api.github.com/repos/jrsoftware/issrc/commits/main', 2 * 1024 * 1024))['sha']
    if not re.fullmatch(r'[0-9a-f]{40}', commit):
        raise ValueError('invalid upstream revision')
    args.output.mkdir(exist_ok=False)
    listings, records = {}, []
    for locale, (directory, stem) in LANGUAGES.items():
        if directory not in listings:
            listings[directory] = json.loads(read_url(
                f'https://api.github.com/repos/jrsoftware/issrc/contents/{directory}?ref={commit}',
                2 * 1024 * 1024))
        names = [item['name'] for item in listings[directory]
                 if item.get('type') == 'file' and item['name'] in (stem + '.isl', stem + '.islu')]
        if len(names) != 1:
            raise ValueError('missing or ambiguous translation: ' + locale)
        name = names[0]
        url = f'https://raw.githubusercontent.com/jrsoftware/issrc/{commit}/{directory}/{urllib.parse.quote(name)}'
        raw = read_url(url, 1024 * 1024)
        with (args.output / name).open('xb') as stream:
            stream.write(raw)
        record = {'locale': locale, 'file': name, 'source_url': url,
                  'sha256': hashlib.sha256(raw).hexdigest(), 'installed': False}
        try:
            text, encoding = decode(raw)
            record.update(compare(reference, parse(text)))
            record['encoding'] = encoding
        except (ValueError, UnicodeError, LookupError) as error:
            record['parse_error'] = str(error)
        records.append(record)
        print(json.dumps(record, ensure_ascii=True), flush=True)
    report = {'upstream_revision': commit, 'reference_sha256': hashlib.sha256(reference_raw).hexdigest(),
              'candidates': records, 'visual_acceptance': False, 'translation_quality_approved': False}
    (args.output / 'audit.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print('INNO_TRANSLATION_CANDIDATE_AUDIT_FINISHED', flush=True)


if __name__ == '__main__':
    main()
