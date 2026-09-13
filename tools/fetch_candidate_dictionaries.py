"""Fetch only allowlisted public data for review; no builds, signing or installation.

The pinned LibreOffice snapshot is the one already used for embedded Estonian.
Downloaded data remains untrusted and is not a release-ready input pack.
"""
import hashlib
import json
from pathlib import Path
import sys
import urllib.request


REVISION = "32b006a2c22a4ac7e8ed3f03346f7b3d85a970a4"
BASE = f"https://raw.githubusercontent.com/LibreOffice/dictionaries/{REVISION}/"
FILES = (
    "hi_IN/hi_IN.dic", "hi_IN/hi_IN.aff", "hi_IN/Copyright",
    "hi_IN/COPYING", "hi_IN/README_hi_IN.txt",
    "bn_BD/bn_BD.dic", "bn_BD/bn_BD.aff", "bn_BD/COPYING",
    "ar/ar.dic", "ar/ar.aff", "ar/COPYING.txt", "ar/AUTHORS.txt", "ar/README_ar.txt",
)
MAX_FILE = 16 * 1024 * 1024
MAX_TOTAL = 64 * 1024 * 1024
WORDFREQ_REVISION = "42233e6c36ce792031bcccfa17cdd0cec9af5fa7"
SOURCES = {
    "cc-by-sa": ("4.0", "https://creativecommons.org/licenses/by-sa/4.0/", ("legalcode.txt",)),
    "hunspell": (REVISION, BASE, FILES),
    "wordfreq": (
        WORDFREQ_REVISION,
        f"https://raw.githubusercontent.com/rspeer/wordfreq/{WORDFREQ_REVISION}/",
        tuple(f"wordfreq/data/large_{lang}.msgpack.gz" for lang in
              ("ar", "bn", "de", "es", "fr", "ja", "pt", "zh"))
        + tuple(f"wordfreq/data/small_{lang}.msgpack.gz" for lang in ("hi", "id", "ur"))
        + ("NOTICE.md", "LICENSE.txt", "README.md"),
    ),
}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("redirect rejected: pinned raw source required")


def fetch(root, source="hunspell"):
    revision, base, files = SOURCES[source]
    # Refuse existing roots. Retain partial output on failure for inspection.
    root.mkdir()
    opener = urllib.request.build_opener(NoRedirect())
    records = []
    total = 0
    for relative in files:
        request = urllib.request.Request(base + relative, headers={
            "User-Agent": "AutoKeyboardLayot-source-audit/1",
            "Accept-Encoding": "identity",
        })
        with opener.open(request, timeout=25) as response:
            if response.status != 200 or response.geturl() != base + relative:
                raise ValueError("unexpected source response")
            payload = response.read(MAX_FILE + 1)
        if not payload or len(payload) > MAX_FILE:
            raise ValueError("empty or oversized source")
        total += len(payload)
        if total > MAX_TOTAL:
            raise ValueError("aggregate source bound exceeded")
        target = root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open("xb") as output:
            output.write(payload)
        records.append({"path": relative, "url": base + relative,
                        "bytes": len(payload), "sha256": hashlib.sha256(payload).hexdigest()})
        print(f"FETCHED {relative} {len(payload)} bytes", flush=True)
    with (root / "FETCH-COMPLETE.json").open("x", encoding="utf-8") as output:
        json.dump({"format": 1, "status": "unreviewed-upstream-candidates",
                   "source": source, "revision": revision, "files": records}, output, indent=2)
        output.write("\n")
    print("CANDIDATE_FETCH_COMPLETE", flush=True)


if __name__ == "__main__":
    if len(sys.argv) not in (2, 3) or (len(sys.argv) == 3 and sys.argv[2] not in SOURCES):
        raise SystemExit("usage: fetch_candidate_dictionaries.py NEW_DIRECTORY [hunspell|wordfreq|cc-by-sa]")
    fetch(Path(sys.argv[1]), sys.argv[2] if len(sys.argv) == 3 else "hunspell")
