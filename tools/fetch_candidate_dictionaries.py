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

# SHA-256 of every file as reviewed on 2026-09-09 (the candidates the packages were built from).
# A file that differs is refused: review the new content and update its hash on purpose.
REVIEWED = {
    "cc-by-sa": {
        "legalcode.txt": "28a9529c7d0bb4dc51f4bf5c116a3d16ef247a052f7591466768ddf563fd1cf5",
    },
    "hunspell": {
        "hi_IN/hi_IN.dic": "1e01f962a02638ef73e3f8de3c44bfd854f7059d31c9fa96cff0b73a2840f9d9",
        "hi_IN/hi_IN.aff": "3ab96772dc3d1cdbec4141798efb8b7a091b92c9acbeb5dfd3c4998a5c508302",
        "hi_IN/Copyright": "67e86e61e452247c2c491b63629799dec1d6dd4cf4aae47548f5141cf3707098",
        "hi_IN/COPYING": "204d8eff92f95aac4df6c8122bc1505f468f3a901e5a4cc08940e0ede1938994",
        "hi_IN/README_hi_IN.txt": "2b373a1e07efe18e89c04c7f69bd540681a5926edcb478a4331d0df387861487",
        "bn_BD/bn_BD.dic": "cfc78b361861a726d22f0654d7c4e0b47f843c4a9e8b605c4c99e91ea683e116",
        "bn_BD/bn_BD.aff": "6beeacefab0f691cb415c9ab8de227091a3be65510c3d8c0479513b261e61b97",
        "bn_BD/COPYING": "204d8eff92f95aac4df6c8122bc1505f468f3a901e5a4cc08940e0ede1938994",
        "ar/ar.dic": "2a3e5367f61c1583734db9d66734f5603e6be5c2d227cf5c5cd7e4ca586e34fe",
        "ar/ar.aff": "cec30b8621001e49618feb05aec1984c5fcfbf7d2ec309901d5cbf66585217a3",
        "ar/COPYING.txt": "73480d918cd78d655508e80aa88c022cbd53634524a0f714b5f2519f2cf19124",
        "ar/AUTHORS.txt": "bade1d2d0d47dff5956c08732cdb9c02432d9b0b8016a56e27f58b8d7e842462",
        "ar/README_ar.txt": "5ead684ae5211ccf615686959e24bd21ef6eebbb6195997529fda08b993f615e",
    },
    "wordfreq": {
        "wordfreq/data/large_ar.msgpack.gz": "aa12410764f1946f21d986a920c289c22accb069d9b287d71b61657005516eec",
        "wordfreq/data/large_bn.msgpack.gz": "d10ed5ca1e61a6bf210324424ee25c53ff71175398833067d810ccc6c826d581",
        "wordfreq/data/large_de.msgpack.gz": "8ef04d3f4a28fb48d800d25fa80a96d90a5f80c8496c9a453f99f887c6ad3c12",
        "wordfreq/data/large_es.msgpack.gz": "14f326b4f68d517f9b8b99c1e26ef56a508d2dc8d0ee7a9e6e8732ddab1aa65e",
        "wordfreq/data/large_fr.msgpack.gz": "6f16cd80b9b66c5698ed002becea83ae30d0584c3205f1cf01714ad562c00c8b",
        "wordfreq/data/large_ja.msgpack.gz": "e6ab743b939c1802fc03791f1d58dee2861e822ceecdcdee3f0e05b863ef26d8",
        "wordfreq/data/large_pt.msgpack.gz": "7d764586bca6262f554d5fa77ad8e6841ef42534776e70558065140853660ce2",
        "wordfreq/data/large_zh.msgpack.gz": "e079d7aaff1cfb92874850cb3ffbc7b611af939d9d671c1cbda932d968467cdd",
        "wordfreq/data/small_hi.msgpack.gz": "92ccbf70297d28e97af55a2a517b95c3d8b599e4cc943fb6d4612a3d44a86992",
        "wordfreq/data/small_id.msgpack.gz": "6fd891027cd6395b5c038c0e6402fa7e107ffd8973f862053246b897beb1bcd3",
        "wordfreq/data/small_ur.msgpack.gz": "2587f23e5974e940e4dbeff64e9d3c918b5bd81b5089020cb6803f3c09626ac6",
        "NOTICE.md": "d9b104dafe63886af55e7cea7484824e32e12d4e1f57f319923dd643b36d6b15",
        "LICENSE.txt": "8990c551671a51765c4dfe9428f3e1d58dc327f49aca346a32d28cc0fcc7aa88",
        "README.md": "0f1db058e1df2e6bda2db5d0ea851a7111d2390ec16a9939d971ef53ea24f684",
    },
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
        digest = hashlib.sha256(payload).hexdigest()
        if digest != REVIEWED[source].get(relative):
            raise ValueError(f"{relative} differs from the reviewed file (sha256 {digest})")
        target = root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open("xb") as output:
            output.write(payload)
        records.append({"path": relative, "url": base + relative,
                        "bytes": len(payload), "sha256": digest})
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
