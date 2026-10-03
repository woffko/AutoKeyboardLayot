import hashlib
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
METADATA = ROOT / 'data/package-signing/public-key.json'
SIGNER = re.compile(r'^[a-z0-9-]{1,32}$')
MAX_BYTES = 4096
MAX_KEYS = 8

FORMAT_1_FIELDS = {'format', 'algorithm', 'signer', 'public_key_hex', 'fingerprint_sha256', 'repository'}
FORMAT_2_FIELDS = {'format', 'algorithm', 'repository', 'keys'}
KEY_FIELDS = {'signer', 'public_key_hex', 'fingerprint_sha256', 'role'}


def listed_keys(record):
    """The keys of a trust record. Format 1 lists one key, which is the release key;
    format 2 lists one to eight keys, each with a role."""
    if record['format'] == 1:
        assert set(record) == FORMAT_1_FIELDS, sorted(record)
        return [{'signer': record['signer'], 'public_key_hex': record['public_key_hex'],
                 'fingerprint_sha256': record['fingerprint_sha256'], 'role': 'release'}]
    assert record['format'] == 2, record['format']
    assert set(record) == FORMAT_2_FIELDS, sorted(record)
    for key in record['keys']:
        assert set(key) == KEY_FIELDS, sorted(key)
    return record['keys']


def no_duplicate_fields(pairs):
    """Python's json keeps the last copy of a repeated field; the Rust parser refuses the file."""
    names = [name for name, _ in pairs]
    assert len(names) == len(set(names)), f'repeated field in {names}'
    return dict(pairs)


class PublicSigningMetadataTests(unittest.TestCase):
    def setUp(self):
        self.raw = METADATA.read_bytes()
        # Plain UTF-8 without a byte-order mark, and no repeated field anywhere.
        self.record = json.loads(self.raw.decode('utf-8'), object_pairs_hook=no_duplicate_fields)

    def test_public_record_is_bounded_and_consistent(self):
        self.assertLessEqual(len(self.raw), MAX_BYTES)
        self.assertIn(self.record['format'], (1, 2))
        self.assertEqual(self.record['algorithm'], 'Ed25519')
        self.assertEqual(self.record['repository'], 'woffko/AutoKeyboardLayot')
        keys = listed_keys(self.record)
        self.assertTrue(1 <= len(keys) <= MAX_KEYS)
        for key in keys:
            self.assertRegex(key['signer'], SIGNER)
            # The key tool records lowercase hex, and so does the published file.
            self.assertRegex(key['public_key_hex'], r'^[0-9a-f]{64}$')
            self.assertRegex(key['fingerprint_sha256'], r'^[0-9a-f]{64}$')
            public = bytes.fromhex(key['public_key_hex'])
            self.assertEqual(len(public), 32)
            self.assertEqual(hashlib.sha256(public).hexdigest(), key['fingerprint_sha256'])
            self.assertIn(key['role'], ('release', 'recovery'))

    def test_signers_and_keys_are_unique_and_exactly_one_key_signs(self):
        keys = listed_keys(self.record)
        self.assertEqual(len({key['signer'] for key in keys}), len(keys))
        self.assertEqual(len({key['public_key_hex'].lower() for key in keys}), len(keys))
        self.assertEqual([key['role'] for key in keys].count('release'), 1)


if __name__ == '__main__':
    unittest.main()
