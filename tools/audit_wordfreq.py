"""Read the pinned cBpack v1 data without running upstream code.

Audit only: no normalization, cutoff, token filtering, or package publication.
Schema: https://github.com/rspeer/wordfreq/blob/42233e6c36ce792031bcccfa17cdd0cec9af5fa7/wordfreq/__init__.py
"""
import gzip
import hashlib
import io
import json
from pathlib import Path
import sys
import unicodedata

from fetch_candidate_dictionaries import SOURCES, WORDFREQ_REVISION

MAX_RAW = 64 * 1024 * 1024
MAX_WORDS = 2_000_000


class Reader:
    def __init__(self, data):
        self.data = data
        self.offset = 0

    def take(self, count):
        end = self.offset + count
        if end > len(self.data):
            raise ValueError("truncated cBpack")
        result = self.data[self.offset:end]
        self.offset = end
        return result

    def number(self, count):
        return int.from_bytes(self.take(count), "big")

    def array(self, limit):
        tag = self.number(1)
        if 0x90 <= tag <= 0x9f:
            count = tag - 0x90
        elif tag in (0xdc, 0xdd):
            count = self.number(2 if tag == 0xdc else 4)
        else:
            raise ValueError("expected array")
        if count > limit:
            raise ValueError("array exceeds bound")
        return count

    def string(self):
        tag = self.number(1)
        if 0xa0 <= tag <= 0xbf:
            size = tag - 0xa0
        elif tag in (0xd9, 0xda, 0xdb):
            size = self.number({0xd9: 1, 0xda: 2, 0xdb: 4}[tag])
        else:
            raise ValueError("expected string")
        if size > 65536:
            raise ValueError("string exceeds audit bound")
        return self.take(size).decode("utf-8", errors="strict")


def decode(data):
    if len(data) > MAX_RAW:
        raise ValueError("decompressed file exceeds bound")
    reader = Reader(data)
    count = reader.array(1002)
    if count < 2 or reader.take(1) != b"\x82":
        raise ValueError("expected cBpack header map")
    header = {}
    for _ in range(2):
        key = reader.string()
        if key in header:
            raise ValueError("duplicate header key")
        header[key] = reader.string() if key == "format" else reader.number(1)
    if header != {"format": "cB", "version": 1}:
        raise ValueError("unexpected cBpack header")
    bins = []
    total = 0
    for _ in range(count - 1):
        size = reader.array(MAX_WORDS - total)
        total += size
        bins.append([reader.string() for _ in range(size)])
    if reader.offset != len(data):
        raise ValueError("trailing data")
    return bins


def audit(root, export_root=None):
    manifest = json.loads((root / "FETCH-COMPLETE.json").read_text())
    if manifest.get("source") != "wordfreq" or manifest.get("revision") != WORDFREQ_REVISION:
        raise ValueError("unexpected source revision")
    expected = SOURCES["wordfreq"][2]
    records = manifest["files"]
    if [record["path"] for record in records] != list(expected):
        raise ValueError("unexpected source inventory")
    if export_root is not None:
        # Lossless developer intermediate, not an importable or signed package.
        # Refuse existing output and retain partial output on failure.
        export_root.mkdir()
    for record in records:
        path = root / record["path"]
        payload = path.read_bytes()
        if len(payload) != record["bytes"] or hashlib.sha256(payload).hexdigest() != record["sha256"]:
            raise ValueError("source integrity mismatch")
        if not path.name.endswith(".msgpack.gz"):
            continue
        with gzip.GzipFile(fileobj=io.BytesIO(payload), mode="rb") as stream:
            raw = stream.read(MAX_RAW + 1)
        bins = decode(raw)
        if export_root is not None:
            with (export_root / (path.name.removesuffix(".msgpack.gz") + ".json")).open("x", encoding="utf-8") as output:
                json.dump({"format": 1, "source_revision": WORDFREQ_REVISION,
                           "source_sha256": record["sha256"], "bins": bins},
                          output, ensure_ascii=False, separators=(",", ":"))
        words = [word for bucket in bins for word in bucket]
        counts = {
            "file": record["path"], "words": len(words), "unique": len(set(words)),
            "raw_bytes": len(raw), "frequency_bins": len(bins),
            "max_chars": max(map(len, words), default=0),
            "max_utf8_bytes": max((len(w.encode()) for w in words), default=0),
            "empty": sum(not w for w in words),
            "over_64_chars_or_256_bytes": sum(len(w) > 64 or len(w.encode()) > 256 for w in words),
            "whitespace_or_control": sum(any(c.isspace() or unicodedata.category(c) == "Cc" for c in w) for w in words),
            "combining_marks": sum(any(unicodedata.category(c).startswith("M") for c in w) for w in words),
            "not_nfc": sum(unicodedata.normalize("NFC", w) != w for w in words),
        }
        print(json.dumps(counts, ensure_ascii=True), flush=True)
    if export_root is not None:
        for name in ("NOTICE.md", "LICENSE.txt", "README.md"):
            with (export_root / name).open("xb") as output:
                output.write((root / name).read_bytes())
        with (export_root / "AUDIT-EXPORT-COMPLETE.json").open("x", encoding="utf-8") as output:
            json.dump({"format": 1, "status": "lossless-audit-only-not-installable",
                       "source_revision": WORDFREQ_REVISION,
                       "frequency": "bin index is negative centibels; no new cutoff",
                       "normalization": "none", "data_license": "CC-BY-SA-4.0; see NOTICE.md",
                       "attribution": "wordfreq, Copyright 2022 Robyn Speer; SUBTLEX authors and other sources credited in NOTICE.md and README.md"}, output, indent=2)
    print("WORDFREQ_AUDIT_COMPLETE", flush=True)


if __name__ == "__main__":
    if len(sys.argv) not in (2, 3):
        raise SystemExit("usage: audit_wordfreq.py FETCHED_DIRECTORY [NEW_AUDIT_EXPORT_DIRECTORY]")
    audit(Path(sys.argv[1]), Path(sys.argv[2]) if len(sys.argv) == 3 else None)
