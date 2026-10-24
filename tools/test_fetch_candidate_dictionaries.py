"""Offline tests: no real requests, source installation or external writes."""
import contextlib
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import fetch_candidate_dictionaries as fetcher


class Response(io.BytesIO):
    status = 200

    def __init__(self, url, payload):
        super().__init__(payload)
        self.url = url

    def geturl(self):
        return self.url


def reviewed(payload):
    """REVIEWED as if every file had been reviewed with the content `payload`."""
    digest = hashlib.sha256(payload).hexdigest()
    return {source: {name: digest for name in files} for source, (_, _, files) in fetcher.SOURCES.items()}


class FetchTests(unittest.TestCase):
    def test_every_allowlisted_file_has_a_reviewed_hash(self):
        for source, (_, _, files) in fetcher.SOURCES.items():
            with self.subTest(source=source):
                self.assertEqual(set(fetcher.REVIEWED[source]), set(files))
                for digest in fetcher.REVIEWED[source].values():
                    self.assertRegex(digest, r"^[0-9a-f]{64}$")

    def test_a_file_that_differs_from_the_reviewed_one_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            destination = Path(temp) / "new"

            def request(req, timeout):
                return Response(req.full_url, b"changed upstream")

            with patch.object(fetcher.urllib.request, "build_opener") as opener, \
                    patch.dict(fetcher.REVIEWED, reviewed(b"fixture")):
                opener.return_value.open.side_effect = request
                with self.assertRaisesRegex(ValueError, "differs from the reviewed file"):
                    fetcher.fetch(destination)
            self.assertFalse((destination / "FETCH-COMPLETE.json").exists())
            self.assertEqual(list(destination.iterdir()), [])

    def test_both_fixed_allowlists_complete_and_refuse_overwrite(self):
        for source in fetcher.SOURCES:
            with self.subTest(source=source), tempfile.TemporaryDirectory() as temp:
                destination = Path(temp) / "new"
                calls = []

                def request(req, timeout):
                    calls.append(req.full_url)
                    self.assertEqual(timeout, 25)
                    return Response(req.full_url, b"fixture")

                with patch.object(fetcher.urllib.request, "build_opener") as opener, \
                        patch.dict(fetcher.REVIEWED, reviewed(b"fixture")):
                    opener.return_value.open.side_effect = request
                    with contextlib.redirect_stdout(io.StringIO()):
                        fetcher.fetch(destination, source)
                    inventory = json.loads((destination / "FETCH-COMPLETE.json").read_text())
                    revision, base, files = fetcher.SOURCES[source]
                    self.assertEqual(inventory["revision"], revision)
                    self.assertEqual(calls, [base + name for name in files])
                    self.assertEqual(len(inventory["files"]), len(files))
                    self.assertTrue(all(row["bytes"] == 7 for row in inventory["files"]))
                    with self.assertRaises(FileExistsError):
                        fetcher.fetch(destination, source)
                    self.assertEqual(len(calls), len(files))

    def test_size_empty_and_redirect_fail_without_completion_marker(self):
        for mode in ("empty", "oversize", "url", "aggregate"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temp:
                destination = Path(temp) / "new"

                def request(req, timeout):
                    return Response("https://example.invalid/" if mode == "url" else req.full_url,
                                    b"" if mode == "empty" else b"abc")

                with patch.object(fetcher.urllib.request, "build_opener") as opener, \
                        patch.object(fetcher, "MAX_FILE", 2 if mode == "oversize" else 4), \
                        patch.object(fetcher, "MAX_TOTAL", 2 if mode == "aggregate" else 64):
                    opener.return_value.open.side_effect = request
                    with self.assertRaises(ValueError):
                        fetcher.fetch(destination)
                self.assertFalse((destination / "FETCH-COMPLETE.json").exists())

    def test_redirect_handler_never_follows(self):
        with self.assertRaises(ValueError):
            fetcher.NoRedirect().redirect_request(None, None, 302, None, None,
                                                  "https://example.invalid/")


if __name__ == "__main__":
    unittest.main()
