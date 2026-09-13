import contextlib
import gzip
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from prepare_wordfreq_sources import normalize, prepare, TRAILING_NNBSP, WORDFREQ_REVISION


class PrepareTests(unittest.TestCase):
    def test_normalization_is_exact_and_never_truncates_long_words(self):
        for language, words in TRAILING_NNBSP.items():
            for word in words:
                self.assertEqual(normalize(language, word), word[:-1])
        for language, word in [("es", "faire\u202f"), ("fr", "unknown\u202f"),
                               ("de", " a "), ("de", "a" * 80), ("bn", "বাংলা")]:
            self.assertEqual(normalize(language, word), word)

    def test_receipt_preserves_count_original_and_notices(self):
        # Minimal schema-valid cBpack containing '00' plus its reviewed variant.
        cbpack = b"\x92\x82\xa6format\xa2cB\xa7version\x01\x92\xa200\xa500\xe2\x80\xaf"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "source"
            root.mkdir()
            payloads = {"large_de.msgpack.gz": gzip.compress(cbpack),
                        "NOTICE.md": b"notice", "LICENSE.txt": b"license", "README.md": b"readme"}
            for name, data in payloads.items():
                (root / name).write_bytes(data)
            (root / "FETCH-COMPLETE.json").write_text(json.dumps({
                "source": "wordfreq", "revision": WORDFREQ_REVISION,
                "files": [{"path": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
                          for name, data in payloads.items()]}))
            target = Path(directory) / "prepared"
            license_path = Path(directory) / "legalcode.txt"
            license_path.write_bytes(b"complete fixture legal text")
            with patch("prepare_wordfreq_sources.SOURCES", {"wordfreq": (None, None, tuple(payloads))}), \
                    patch("prepare_wordfreq_sources.CC_LICENSE_SHA256", hashlib.sha256(license_path.read_bytes()).hexdigest()), \
                    contextlib.redirect_stdout(io.StringIO()):
                prepare(root, target, license_path)
                self.assertEqual((target / "de/words.txt").read_bytes(), b"00\n00\n")
                report = json.loads((target / "de/SOURCE.json").read_text())
                self.assertEqual((report["input_records"], report["output_records"], report["output_unique"], report["removed_records"]), (2, 2, 1, 0))
                self.assertEqual(report["normalization"][0]["original"], "00\u202f")
                self.assertEqual((target / "de/original.msgpack.gz").read_bytes(), payloads["large_de.msgpack.gz"])
                self.assertEqual((target / "NOTICE.md").read_bytes(), b"notice")
                for name in ("NOTICE.md", "LICENSE.txt", "README.md"):
                    self.assertEqual((target / "de" / name).read_bytes(), payloads[name])
                self.assertEqual((target / "de/CC-BY-SA-4.0.txt").read_bytes(), license_path.read_bytes())
                attribution = (target / "de/ATTRIBUTION.txt").read_text()
                self.assertIn(WORDFREQ_REVISION, attribution)
                self.assertIn("No records", attribution)
                self.assertIn("CC-BY-SA-4.0.txt", report["data_license"])
                self.assertEqual(len(report["files"]), 7)
                for record in report["files"]:
                    data = (target / "de" / record["path"]).read_bytes()
                    self.assertEqual(record["bytes"], len(data))
                    self.assertEqual(record["sha256"], hashlib.sha256(data).hexdigest())
                with self.assertRaises(FileExistsError):
                    prepare(root, target, license_path)
                original_notice = (root / "NOTICE.md").read_bytes()
                (root / "NOTICE.md").write_bytes(b"bad notice")
                with self.assertRaises(ValueError):
                    prepare(root, Path(directory) / "bad-notice", license_path)
                self.assertFalse((Path(directory) / "bad-notice").exists())
                (root / "NOTICE.md").write_bytes(original_notice)
                original_license = license_path.read_bytes()
                for data in (b"bad legal text", b"x" * (64 * 1024 + 1)):
                    license_path.write_bytes(data)
                    with self.assertRaises(ValueError):
                        prepare(root, Path(directory) / "bad-license", license_path)
                    self.assertFalse((Path(directory) / "bad-license").exists())
                license_path.write_bytes(original_license)
                (root / "large_de.msgpack.gz").write_bytes(b"bad")
                failed = Path(directory) / "failed"
                with self.assertRaises(ValueError):
                    prepare(root, failed, license_path)
                self.assertFalse((failed / "SOURCE-COMPLETE.json").exists())


if __name__ == "__main__":
    unittest.main()
