import unittest
import contextlib
import gzip
import hashlib
import io
import json
from pathlib import Path
import tempfile
from unittest.mock import patch

from audit_wordfreq import audit, decode, WORDFREQ_REVISION


HEADER = b"\x82\xa6format\xa2cB\xa7version\x01"


class DecodeTests(unittest.TestCase):
    def test_complete_bins_and_unicode(self):
        self.assertEqual(decode(b"\x93" + HEADER + b"\x90\x92\xa1a\xa2\xc3\xa9"), [[], ["a", "é"]])

    def test_array_and_string_encodings(self):
        for array in (b"\x91", b"\xdc\x00\x01", b"\xdd\x00\x00\x00\x01"):
            for string in (b"\xa1a", b"\xd9\x01a", b"\xda\x00\x01a", b"\xdb\x00\x00\x00\x01a"):
                self.assertEqual(decode(b"\x92" + HEADER + array + string), [["a"]])

    def test_rejects_truncation_at_every_byte(self):
        valid = b"\x92" + HEADER + b"\x91\xa1a"
        for end in range(len(valid)):
            with self.subTest(end=end), self.assertRaises(ValueError):
                decode(valid[:end])

    def test_rejects_wrong_schema_bounds_and_trailing_bytes(self):
        cases = [
            b"\x92" + HEADER + b"\x90\x00",
            b"\x92" + HEADER.replace(b"cB", b"xx") + b"\x90",
            b"\x92" + HEADER[:-1] + b"\x02\x90",
            b"\x92" + HEADER + b"\xdd\xff\xff\xff\xff",
            b"\x92" + HEADER + b"\x91\xdb\xff\xff\xff\xff",
            b"\x92" + HEADER + b"\x91\xa1\xff",
            b"\x92" + HEADER + b"\x91\x91\xa1a",
            b"\xdc\xff\xff",
            b"\x91" + HEADER,
        ]
        for case in cases:
            with self.subTest(case=case), self.assertRaises(ValueError):
                decode(case)

    def test_export_preserves_every_bin_and_notices_and_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "source"
            root.mkdir()
            payloads = {"large_ar.msgpack.gz": gzip.compress(b"\x93" + HEADER + b"\x90\x92\xa1a\xa2\xc3\xa9"),
                        "NOTICE.md": b"attribution", "LICENSE.txt": b"license", "README.md": b"readme"}
            for name, data in payloads.items():
                (root / name).write_bytes(data)
            (root / "FETCH-COMPLETE.json").write_text(json.dumps({
                "source": "wordfreq", "revision": WORDFREQ_REVISION,
                "files": [{"path": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
                          for name, data in payloads.items()]}))
            target = Path(directory) / "export"
            with patch("audit_wordfreq.SOURCES", {"wordfreq": (None, None, tuple(payloads))}), contextlib.redirect_stdout(io.StringIO()):
                audit(root, target)
                self.assertEqual(json.loads((target / "large_ar.json").read_text())["bins"], [[], ["a", "é"]])
                self.assertEqual((target / "NOTICE.md").read_bytes(), b"attribution")
                before = {p.name: p.read_bytes() for p in target.iterdir()}
                with self.assertRaises(FileExistsError):
                    audit(root, target)
                self.assertEqual(before, {p.name: p.read_bytes() for p in target.iterdir()})
                (root / "large_ar.msgpack.gz").write_bytes(b"corrupt")
                failed_target = Path(directory) / "failed"
                with self.assertRaises(ValueError):
                    audit(root, failed_target)
                self.assertFalse((failed_target / "AUDIT-EXPORT-COMPLETE.json").exists())


if __name__ == "__main__":
    unittest.main()
