"""Prepare complete unsigned dictionary sources with explicit normalization receipts.

No frequency cutoff, script filtering, short-word selection, signing, or installation.
Preserves original compressed data and upstream notices beside every derivative.
"""
import gzip
import hashlib
import io
import json
from pathlib import Path
import sys

from audit_wordfreq import decode, MAX_RAW
from fetch_candidate_dictionaries import SOURCES, WORDFREQ_REVISION, MAX_FILE

CC_LICENSE_SHA256 = "28a9529c7d0bb4dc51f4bf5c116a3d16ef247a052f7591466768ddf563fd1cf5"
CC_LICENSE_URL = "https://creativecommons.org/licenses/by-sa/4.0/legalcode.txt"
NOTICES = ("NOTICE.md", "LICENSE.txt", "README.md")


def read_bounded(path, limit):
    with path.open("rb") as stream:
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise ValueError("source exceeds bound")
    return data


def verified_source(root, record):
    payload = read_bounded(root / record["path"], MAX_FILE)
    if len(payload) != record["bytes"] or hashlib.sha256(payload).hexdigest() != record["sha256"]:
        raise ValueError("source integrity mismatch")
    return payload


# Exact pinned-source defects reviewed in the full production-API audit.
# Do not broaden to arbitrary strip(): embedded whitespace remains an error.
TRAILING_NNBSP = {
    "de": {"00\u202f"},
    "fr": {"0000\u202f", "00\u202f", "faire\u202f", "non\u202f",
           "monde\u202f", "ça\u202f", "000\u202f", "là\u202f"},
}


def normalize(language, word):
    if word in TRAILING_NNBSP.get(language, set()):
        return word[:-1]
    return word


def write_new(path, payload):
    with path.open("xb") as output:
        output.write(payload)
    return {"path": path.name, "bytes": len(payload), "sha256": hashlib.sha256(payload).hexdigest()}


def prepare(root, destination, license_path):
    manifest = json.loads(read_bounded(root / "FETCH-COMPLETE.json", 64 * 1024))
    expected = SOURCES["wordfreq"][2]
    if manifest.get("source") != "wordfreq" or manifest.get("revision") != WORDFREQ_REVISION:
        raise ValueError("unexpected source")
    records = manifest["files"]
    if [record["path"] for record in records] != list(expected):
        raise ValueError("unexpected inventory")
    legalcode = read_bounded(license_path, 64 * 1024)
    if hashlib.sha256(legalcode).hexdigest() != CC_LICENSE_SHA256:
        raise ValueError("CC-BY-SA-4.0 legal text does not match reviewed SHA-256")
    # Snapshot verified notices before producing any standalone derivative.
    notices = {name: verified_source(root, next(r for r in records if r["path"] == name))
               for name in NOTICES}
    destination.mkdir()
    for name, payload in notices.items():
        write_new(destination / name, payload)
    write_new(destination / "CC-BY-SA-4.0.txt", legalcode)
    for record in records:
        path = root / record["path"]
        if not path.name.endswith(".msgpack.gz"):
            continue
        payload = verified_source(root, record)
        language = path.name.split(".")[0].split("_")[1]
        with gzip.GzipFile(fileobj=io.BytesIO(payload), mode="rb") as stream:
            raw = stream.read(MAX_RAW + 1)
        bins = decode(raw)
        words = []
        changes = []
        for bin_index, bucket in enumerate(bins):
            for word in bucket:
                normalized = normalize(language, word)
                if normalized != word:
                    changes.append({"original": word, "normalized": normalized,
                                    "negative_centibels": bin_index,
                                    "reason": "reviewed trailing U+202F source artifact"})
                if not normalized or len(normalized.encode()) > 256 or any(c.isspace() or ord(c) < 32 or 127 <= ord(c) < 160 for c in normalized):
                    raise ValueError("unexpected invalid token; no automatic filtering")
                words.append(normalized)
        if {change["original"] for change in changes} != TRAILING_NNBSP.get(language, set()):
            raise ValueError("reviewed normalization set does not match source")
        package_root = destination / language
        package_root.mkdir()
        files = [write_new(package_root / "original.msgpack.gz", payload),
                 write_new(package_root / "words.txt", ("\n".join(words) + "\n").encode())]
        for name, notice in notices.items():
            files.append(write_new(package_root / name, notice))
        files.append(write_new(package_root / "CC-BY-SA-4.0.txt", legalcode))
        attribution = (
            "Dictionary data derived from wordfreq, Copyright 2022 Robyn Speer.\n"
            "Data license: Creative Commons Attribution-ShareAlike 4.0 International.\n"
            f"License text: {CC_LICENSE_URL}\n"
            f"Source: https://github.com/rspeer/wordfreq/tree/{WORDFREQ_REVISION}\n"
            f"Source file: {record['path']}\n"
            "Retain NOTICE.md and README.md for SUBTLEX and all other source credits,\n"
            "license details and upstream modifications. LICENSE.txt applies to upstream\n"
            "code, not a replacement for the separate data license.\n"
            "Local changes: cBpack records converted to newline-delimited UTF-8; only\n"
            "the exact per-record normalizations in SOURCE.json were applied. No records\n"
            "removed; original compressed frequency data retained unchanged.\n"
            "Unsigned developer source only; not an installable or accepted input pack.\n"
        ).encode()
        files.append(write_new(package_root / "ATTRIBUTION.txt", attribution))
        report = {"format": 1, "status": "unsigned-incomplete-source-not-installable",
                  "language": language, "source": record, "source_revision": WORDFREQ_REVISION,
                  "input_records": sum(map(len, bins)), "output_records": len(words),
                  "output_unique": len(set(words)), "removed_records": 0,
                  "normalization": changes, "frequency_data": "retained in original.msgpack.gz; no cutoff added",
                  "files": files, "data_license": "CC-BY-SA-4.0; see local CC-BY-SA-4.0.txt, NOTICE.md, README.md and ATTRIBUTION.txt",
                  "license_source": {"url": CC_LICENSE_URL, "sha256": CC_LICENSE_SHA256},
                  "pending": ["short-word policy", "scoring and input rules", "language-specific acceptance", "release packaging"]}
        write_new(package_root / "SOURCE.json", (json.dumps(report, ensure_ascii=True, indent=2) + "\n").encode())
        print(f"PREPARED {language} input={report['input_records']} output={len(words)} normalized={len(changes)} removed=0", flush=True)
    write_new(destination / "SOURCE-COMPLETE.json", (json.dumps({
        "format": 1, "status": "unsigned-incomplete-source-not-installable",
        "source_revision": WORDFREQ_REVISION,
        "attribution": "wordfreq, Copyright 2022 Robyn Speer; SUBTLEX authors and all other sources in NOTICE.md and README.md",
        "data_license": "CC-BY-SA-4.0; upstream notices preserved",
        "normalization": "Only the nine explicitly reviewed trailing U+202F records; per-language SOURCE.json contains receipts; zero dropped records"
    }, indent=2) + "\n").encode())


if __name__ == "__main__":
    if len(sys.argv) != 4:
        raise SystemExit("usage: prepare_wordfreq_sources.py FETCHED_DIRECTORY NEW_SOURCE_DIRECTORY CC_BY_SA_LEGALCODE_FILE")
    prepare(Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]))
