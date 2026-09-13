"""Offline tests: no real requests, source installation or external writes."""
import contextlib
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


class FetchTests(unittest.TestCase):
    def test_both_fixed_allowlists_complete_and_refuse_overwrite(self):
        for source in fetcher.SOURCES:
            with self.subTest(source=source), tempfile.TemporaryDirectory() as temp:
                destination = Path(temp) / "new"
                calls = []

                def request(req, timeout):
                    calls.append(req.full_url)
                    self.assertEqual(timeout, 25)
                    return Response(req.full_url, b"fixture")

                with patch.object(fetcher.urllib.request, "build_opener") as opener:
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
