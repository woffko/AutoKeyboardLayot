import hashlib
import json
from pathlib import Path
import unittest


class PublicSigningMetadataTests(unittest.TestCase):
    def test_public_record_is_bounded_and_consistent(self):
        root = Path(__file__).resolve().parents[1]
        record = json.loads((root / 'data/package-signing/public-key.json').read_text(encoding='utf-8'))
        self.assertEqual(set(record), {'format', 'algorithm', 'signer', 'public_key_hex',
                                      'fingerprint_sha256', 'repository'})
        self.assertEqual(record['format'], 1)
        self.assertEqual(record['algorithm'], 'Ed25519')
        self.assertRegex(record['signer'], r'^[a-z0-9-]{1,32}$')
        self.assertEqual(record['repository'], 'woffko/AutoKeyboardLayot')
        public = bytes.fromhex(record['public_key_hex'])
        self.assertEqual(len(public), 32)
        self.assertEqual(hashlib.sha256(public).hexdigest(), record['fingerprint_sha256'])


if __name__ == '__main__':
    unittest.main()
