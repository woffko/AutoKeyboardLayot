# Real input-data migration audit

This is a source/build audit, not physical typing or redistribution acceptance.
The separate `AutoKeyboardLayot-language-packs` source project currently contains
13 UI catalog drafts and unsigned RU/ET drafts under `input-sources/`.
These are source data, not installable signed packs. UI coverage is not input coverage.

## Reproduce exact embedded dictionary sizing

```sh
cargo run --offline --locked --example audit_embedded_dictionaries
cargo test --offline --locked --example audit_embedded_dictionaries
```

The read-only example streams the FSTs produced by the actual `build.rs` and
reports word count, UTF-8 byte count, maximum word length, and SHA-256 of sorted
words with one LF after every word. It does not scan user dictionaries or export,
sign, install, enable, or publish anything. Its hash identifies normalized word
data, not the original source archive or a signed package envelope.

Observed on 2026-09-09:

| Tier | Words | UTF-8 bytes including LF | Maximum characters |
| --- | ---: | ---: | ---: |
| en-US | 370,079 | 3,864,753 | 31 |
| ru-RU | 1,436,545 | 33,008,809 | 28 |
| et-EE | 282,036 | 3,174,229 | 26 |
| en-US-short | 104 | 389 | 3 |
| ru-RU-short | 97 | 619 | 3 |
| et-EE-short | 25 | 92 | 3 |

The full Russian tier could not pass the original external dictionary limits of
500,000 words and 16 MiB, or the package role's 16 MiB bound. English and Estonian
fit these numeric limits; this is not a signed-package verification result.
Do not silently truncate, sample, or replace Russian data to make it fit.
The main tier now permits 1,500,000 rows/32 MiB, while the short tier retains
500,000 rows/16 MiB. Both input and lowercased bytes are bounded before
deduplication, including duplicate rows. Transport and aggregate installation
limits are unchanged. Exact-limit and one-over tests cover row/byte limits,
including lowercase UTF-8 expansion. A separate ignored full-corpus signed
round-trip test is run explicitly in release validation, not counted as an
ordinary passing unit test until that explicit run succeeds.
The Russian normalized payload is below 32 MiB; the existing complete envelope
limit is 64 MiB. Neither bound describes peak decoding/compilation memory.

### Full Russian signed round-trip result

The explicit release test passed on 2026-09-09. It generated an in-memory package
using a public synthetic test signer, verified its signature and every component,
and checked membership of all 1,436,545 original words after external compilation.
The short tier, input descriptor, scoring model, and complete Russian license
notice were also retained. No release artifact or production key was created.

- Signed envelope: 34,450,139 bytes.
- Verification/compilation time in this run: 1,388 ms.
- Complete test time: 2.46 seconds.
- Linux process maximum RSS: 193,552 KiB (about 189 MiB), measured with
  `/usr/bin/time -v` on the prebuilt release test executable, excluding compilation
  by Cargo but **including fixture construction**.

This is one real-corpus measurement, not a worst-case memory guarantee, an
aggregate-store stress test, or a Windows runtime measurement. The normalized
representation remains a `BTreeSet`: an unmeasured switch to `Vec` sorting was not
needed to establish compatibility and could increase duplicate-heavy memory use.

## Existing provenance and semantics

### Source bootstrap exporter

`cargo run --offline --locked --example export_input_sources -- NEW_DIRECTORY`
creates an unsigned RU/ET source draft in a directory that must not exist.
It streams the actual built FSTs into sorted UTF-8/LF word files, copies the
allowlisted input/scoring data, original dictionary snapshots and full upstream
notices, and writes `SOURCE-COMPLETE.json` last with every payload's size/hash.
The marker is a reproducibility inventory, not a signature or authorization.
A failure retains the partial directory without a completion marker; use a new
destination after inspection. Existing destinations/files are never overwritten.
Do not run against an output parent writable by untrusted concurrent processes.

The full export on 2026-09-09 reproduced the audited RU and ET word hashes
exactly. Two independent exports (local staging and the separate pack source
project) were byte-for-byte identical, including the completion inventory.
Two exporter tests and strict all-target Linux Clippy passed. This does
not change the runtime embedded packs or establish redistribution rights for
project-maintained short tiers/rules. No key, installed package or release is
created. English remains embedded and is not included in this optional-data export.
The subsequent offline Windows release all-target build and strict Clippy also
passed (81 seconds total). This verifies compilation, not Windows runtime typing.

The exact source snapshots, source hashes, and license notice locations are in
[`data/language-packs/README.md`](../data/language-packs/README.md).
Complete notices must travel with derived data; a manifest's nonempty license
field alone does not establish redistribution permission.

EN/RU are expanded UTF-8 surface lists supplied as gzip files. ET is an
ISO-8859-15 Hunspell dictionary. The current build removes the ET flags and uses
only base entries; it **does not expand `.aff` rules**. Preserve this behavior
during initial migration and retain original `.dic`, `.aff`, and complete
EKI/LGPL notices in source provenance. Do not describe the 282,036 normalized
base entries as the upstream dictionary's roughly 15 million inflected forms.

Input requirements and confidence data exist only for EN-US, RU-RU, and ET-EE:
`data/input/` and `data/scoring/`. Copying a scoring model to a different language
is not a substitute for language-specific evidence. ET requires AltGr/dead-key
support in addition to physical keys; a valid data file does not grant readiness.

## Coverage still required by milestones 5 and 6

| Language/variant | Existing input data | Remaining data/adapter gate |
| --- | --- | --- |
| English / US | Real embedded dictionary, short tier, rules | Retain embedded base; native acceptance |
| Russian / standard | Real embedded dictionary; full signed round-trip passed; separate source draft | Release packaging/licensing; native acceptance |
| Estonian / standard | Base-entry dictionary, short tier, rules; separate source draft | Release packaging/licensing; AltGr/dead-key acceptance |
| Chinese | No input data | Choose explicit script/input method; sourced dictionary/rules; composition adapter |
| Spanish | No input data | Choose exact regional layout; sourced dictionary/rules; adapter acceptance |
| Hindi | No input data | Choose input method; sourced dictionary/rules; complex-script handling |
| Arabic | No input data | Choose exact profile; sourced dictionary/rules; script handling |
| French | No input data | Choose exact regional layout; sourced dictionary/rules; dead-key handling |
| Portuguese | No input data | Choose exact regional layout; sourced dictionary/rules; dead-key handling |
| Bengali | No input data | Choose input method; sourced dictionary/rules; complex-script handling |
| Indonesian | No input data | Choose exact profile; sourced dictionary/rules; same-script ambiguity tests |
| Urdu | No input data | Choose input method; sourced dictionary/rules; script handling |
| German | No input data | Sourced dictionary/rules; exact-profile and dead-key acceptance |
| Japanese | Legacy IME placeholder, no dictionary | Choose exact IME profile; sourced data; composition adapter |

All new sources need pinned provenance and complete terms before distribution.
Production signing authority, repository publication, and physical acceptance
remain separate gates. No production key was generated or enrolled by this audit.
