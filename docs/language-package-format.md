# Signed language-package format 1 (development)

This document specifies the implemented in-memory verifier, not a completed
installer or published package ecosystem. No production signing keys are enrolled.
The default trust store accepts no package. Test signing seeds are public synthetic
fixtures and must never be used for release signing.

## Envelope

One `.aklp` file is a bounded UTF-8 JSON document, at most 64 MiB. Its fields are
`format` (1), `signer` (a trusted key identifier), `manifest` (a JSON string),
`signature` (128 hexadecimal characters) and `components` (fixed-role strings).
There is no ZIP/tar extraction, arbitrary filename, executable plugin, decompressor,
or path interpretation. Unknown and duplicate object fields are rejected.

The signed message is the bytes of `AutoKeyboardLayot.language-package.v1`, one
NUL byte, then the exact UTF-8 bytes of the decoded `manifest` string. The author
serializes the manifest once and signs those exact bytes; readers do not
reserialize or normalize it. Signatures use Ed25519 strict verification, not
batch verification or the legacy-compatibility feature. See the upstream
[strict-verification documentation](https://docs.rs/ed25519-dalek/3.0.0/ed25519_dalek/struct.VerifyingKey.html#method.verify_strict).
Trust anchors are supplied independently by application/release policy, never
by the package. The verifier permits at most eight named public keys and rejects
weak keys and duplicate key identifiers.

## Manifest and component roles

The manifest is at most 32 KiB and has these fields:

- `format`: 1.
- `package_id`: stable case-insensitive `PackId`.
- `revision`: positive unsigned 64-bit integer; an independently versioned package
  revision, not a declaration that a downgrade is permitted.
- `runtime_api`: minimum supported consumer contract. New producers emit 2;
  current consumers accept 1 and 2. API-1 consumers reject API-2 packages before
  installation. API 2 includes the expanded settings and error-message catalog.
  A release catalog must copy the value from each verified package, never
  advertise a lower requirement. Future catalog-ID/placeholder changes require
  an explicit consumer-compatibility review before reusing an API number.
- `input_pack`: optional stable input-pack identity; when present, it must equal
  `package_id` after canonicalization. A package cannot impersonate another input ID.
- `ui_locale`: optional canonical UI catalog identity.
- `components`: fixed roles; each present role has `bytes` and `sha256` fields.

The envelope's matching `components` object contains the actual UTF-8 string for
each role. Hashes are SHA-256 over decoded string bytes, with no newline or Unicode
normalization. Declared byte lengths and hashes must match exactly. Extra,
missing and unrecognized components fail verification.

| Role | Maximum decoded bytes | Requirement |
| --- | ---: | --- |
| `words` | 32 MiB | All four input roles required with `input_pack` |
| `short_words` | 16 MiB | One UTF-8 word per line, 1-3 characters; empty short-word tier allowed |
| `scoring` | 64 KiB | Scoring-model format 1, 2, 3 or 4; see below |
| `input` | 16 KiB | Existing exact-profile descriptor format 1; ID must match |
| `ui` | 512 KiB | Required with `ui_locale`; existing catalog validation |
| `license` | 512 KiB | Required nonempty license text |
| `notice` | 512 KiB | Required nonempty provenance/attribution text |

Input-only, UI-only and combined packages are supported. At least one functional
component must exist. English UI cannot be replaced; a UI locale must match the
catalog's canonical identity, not the built-in fallback or an alias. UI validation
preserves per-message English fallback; it does not establish translation quality.
Input word data is compiled into owned FSTs outside hooks after authentication;
raw external FSTs are not loaded. The main tier permits at most 1,500,000 input
rows and 32 MiB of word bytes; the short tier retains 500,000 rows and 16 MiB.
Both original and lowercased bytes are counted before deduplication. Per-word
storage is limited to 256 UTF-8 bytes, independently of the input session's
unchanged 64-character current-word limit. This retains long dictionary compounds
without expanding buffered input or granting conversion readiness. Scoring and
profile bounds remain in force. Profile requirements never grant OS or adapter
readiness. License text presence/authentication is not legal approval to distribute
third-party data; source provenance and license review remain release gates.

### Scoring-model versions

Format 1 retains the original EN/RU/ET scoring behavior. Format 2 has the same
`ranges`, `vowels`, `bigrams`, `trigrams`, and `rare` fields plus a required
`policy` object. Every policy field is an explicit boolean:

```json
{"bigrams": false, "trigrams": false, "vowels": false, "statistical_targets": false}
```

Disabling a feature removes both its hit bonus and its missing-evidence penalty.
A disabled feature requires an empty corresponding data field; an enabled feature
requires nonempty data. Statistical automatic targets require at least one enabled
n-gram feature. With `statistical_targets=false`, an automatic target must be in
the dictionary or applicable user lexicon; all existing short-word, ambiguity,
score and input-capability gates still apply. Explicit forced conversion keeps
its existing separate policy. Length alone cannot opt a format-2 model into
statistical automatic correction.

Both versions retain the same bounds and normalized alphabetic-scalar validation.
This revision does not add combining-mark/grapheme support, establish linguistic
quality, select a keyboard layout, or enable any IME adapter. Unknown versions,
unknown/duplicate fields, missing policy booleans and contradictory policy/data
are rejected. Format 1 rejects the new policy field entirely. Older binaries
reject format 2; this remains an unpublished development format, not a claim of
backward readability by those binaries.

Format 3 adds required `marks`, a string of distinct lowercase-stable Unicode
scalars, bounded to 1024 UTF-8 bytes (not 1024 characters). Empty is allowed.
Each scalar must have General Category Mn, Mc or Me using the pinned
`unicode-general-category` 1.1.0 Unicode 16.0 tables. Its `ranges` retain the
alphabetic/lowercase checks but must exclude these mark categories: even an
Alphabetic mark must be explicitly listed in `marks`. Formats 1/2 keep their
original range validation and reject the `marks` field.

Format 4 replaces `ranges` with required nonempty `letters`: distinct,
lowercase-stable alphabetic scalars excluding Mn/Mc/Me, bounded to 32768 UTF-8
bytes. It preserves sparse repertoires such as observed Han characters without
filling unobserved gaps. Membership uses an immutable ordered set. `ranges` is
rejected in format 4; `letters` is rejected in formats 1–3. The 64 KiB raw JSON
bound, required `marks`, policy rules and feature limits remain unchanged.
Neither this representation nor successful parsing grants an input capability.

The admitted repertoire is the union of ranges (or format-4 letters) and explicit marks. Ngrams and
rare sequences use that repertoire; a dependent vowel sign may be explicitly
listed in `vowels`, but no accepted mark is automatically treated as a vowel.
The format-2 policy consistency rules apply unchanged. Ngram widths, length
scores, short-word thresholds and buffer limits still count Unicode scalars,
not graphemes or orthographic syllables. Language models must be calibrated for
those units; this format alone does not establish linguistic correctness.

An explicit mark cannot start a scoring token. Automatic and forced target
validation enforce this rule; character buffering may transiently accept an
initial mark because its repertoire query has no preceding-word context. Forced
conversion retains its separate source policy, and all edits still pass the
existing adapter/transaction checks. This repertoire check is not UAX word
segmentation, normalization, a script-validity check, ZWJ/ZWNJ support, or an IME
capability grant. See [composition boundaries](composition-adapter-boundaries.md).

## Boundaries before installation

The verifier returns validated immutable data and the SHA-256 of the entire raw
envelope. That hash identifies exact transport bytes, not a canonical semantic
identity: envelope whitespace/key order may change without invalidating the
manifest signature. The authenticated release catalog must pin the exact raw
artifact hash; package identity/revision must be checked separately. It does not
fetch URLs, activate layouts, install files or enable input.
The manager still must bind an explicit user selection to the authenticated
catalog's exact package ID/revision/length/hash, reject unnoticed downgrades,
enforce ownership/collision rules across packages, and commit installation or
rollback atomically. A valid signature alone is not freshness or an update policy.
Signed release-catalog metadata, local import, selected downloads, durable receipts,
and explicit previous-version rollback now have source implementations; see the
adjacent catalog, inventory, store, and manager documents. Production signing-key
provisioning, published real input packs, and native acceptance remain pending.
The [real-data migration audit](input-pack-migration-audit.md) records why the
development main-tier limits were raised from 500,000 rows/16 MiB: the complete
embedded Russian corpus needs 1,436,545 rows and 33,008,809 bytes with LF separators.
Older development binaries retain their smaller bounds and reject this corpus;
do not advertise those binaries as compatible with the larger package. The
envelope/schema and runtime API number have not changed in this unpublished format.

The verifier decodes one entire bounded JSON document in memory. The 64 MiB input
limit is not a 64 MiB peak-memory guarantee: escaped-string decoding, owned strings
and compiled dictionaries require additional memory. The future loader must bound
aggregate package bytes and avoid concurrent full-size decodes in the input worker.
