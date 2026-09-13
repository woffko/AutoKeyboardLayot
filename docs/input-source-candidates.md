# Additional input-source candidates

Source discovery on 2026-09-09. This is not release licensing approval, input
profile selection, a confidence model, or physical adapter acceptance.

## First bounded candidate fetch

`tools/fetch_candidate_dictionaries.py NEW_DIRECTORY` retrieves only 13
allowlisted raw data/notice files from LibreOffice dictionaries snapshot
`32b006a2c22a4ac7e8ed3f03346f7b3d85a970a4` (the existing Estonian source pin).
It refuses an existing output directory, rejects redirects, limits each file to
16 MiB and all files to 64 MiB, and writes a completion inventory with exact
URLs, lengths and SHA-256 hashes. No upstream scripts are downloaded/executed;
no private files are uploaded. A failed fetch leaves incomplete output without
the completion marker. Use a trusted destination parent, inspect failures and
choose a fresh directory rather than overwriting partial data.

The fixed pin is for reproducibility, not a claim that this is the latest or
best-quality version. Until downloaded notices and data have been inspected,
the output is explicitly `unreviewed-upstream-candidates`.

The first fetch succeeded: all 13 files were retained under the local build
output `target/upstream-candidates-20260909`, with no install/publish step.
Observed dictionary hashes:

| Source | Bytes | SHA-256 |
| --- | ---: | --- |
| hi_IN.dic | 5,004,835 | `1e01f962a02638ef73e3f8de3c44bfd854f7059d31c9fa96cff0b73a2840f9d9` |
| bn_BD.dic | 2,237,684 | `cfc78b361861a726d22f0654d7c4e0b47f843c4a9e8b605c4c99e91ea683e116` |
| ar.dic | 7,217,161 | `2a3e5367f61c1583734db9d66734f5603e6be5c2d227cf5c5cd7e4ca586e34fe` |

Inspection found additional gates: the Hindi affix header credits a newer
upstream than the old Aspell README, so provenance must reconcile both rather
than copying only the oldest notice. Hindi's original `Copyright` is not plain
UTF-8; retain its bytes and explicitly record any presentation transcoding.
Bengali carries an author header and a separate GPL text. Arabic's COPYING is a
short tri-license declaration, not the complete referenced license texts.
Arabic also has comment rows and a nontrivial dictionary count header; a simple
newline parser is not an importer for this file.

No local `hunspell`, `wordforms`, `unmunch` or Hunspell shared library was found
in the checked executable/library paths. Upstream
[`wordforms`](https://github.com/hunspell/hunspell/blob/master/src/tools/wordforms)
uses shared `/tmp/wordforms` files and single-word generation; do not execute it
unmodified as a concurrent bulk compiler. Source rules and expansion correctness
must be reviewed before producing a flat release dictionary.

- Hindi: [`hi_IN`](https://github.com/LibreOffice/dictionaries/tree/master/hi_IN)
  provides `.dic`, `.aff`, `Copyright`, `COPYING` and `README_hi_IN.txt`.
  The [copyright notice](https://github.com/LibreOffice/dictionaries/blob/master/hi_IN/Copyright)
  identifies the word list and GPL version 2 or later.
- Bengali: [`bn_BD`](https://github.com/LibreOffice/dictionaries/tree/master/bn_BD)
  supplies a Bangladesh dictionary/affix pair and `COPYING`. It must not be
  silently relabelled as every Bengali regional/input-method variant.
- Arabic: [`ar`](https://github.com/LibreOffice/dictionaries/tree/master/ar)
  supplies dictionary/affix files, author information and copying/readme notices.
  A generic source directory does not select a Windows keyboard profile.

All three require checking Unicode marks and shaping/composition boundaries.
Flat base-entry extraction is not equivalent to applying Hunspell inflections,
compound rules and exclusions. Do not copy English/Russian confidence models or
silently drop unsupported words to make a corpus load.

## Composition and alternate-source findings

- Chinese: [Rime Luna Pinyin](https://github.com/rime/rime-luna-pinyin) is an
  input-method dictionary/schema candidate, with an
  [LGPL-3 license file](https://github.com/rime/rime-luna-pinyin/blob/master/LICENSE).
  Its dictionary is not a newline list of keyboard-layout corrections.
  Script conversion, pronunciation and active composition are separate concerns;
  do not import or execute upstream YAML schema logic in this data-only agent.
- Japanese: [Mozc](https://github.com/google/mozc) has open-source dictionary
  data, but its [dictionary notice](https://github.com/google/mozc/blob/master/src/data/dictionary_oss/README.txt)
  describes mixed origins including IPAdic and Okinawa data. The engine's BSD
  license alone is not the dictionary notice. No Mozc binaries are to be loaded
  into the keyboard hook as a data-pack shortcut.
- Urdu: [Maḵẖzan](https://github.com/zeerakahmed/makhzan#license--copyright)
  distinguishes restricted raw `/text` from MIT-licensed analyses in `/stats`.
  Do not redistribute its raw texts. Aggregated statistics are a candidate for
  later bounded review, not an already validated spellchecker or input profile.

Spanish, French, Portuguese, German and Indonesian source discovery is also
underway; immutable pins, complete terms and normalization semantics remain to
be verified before adding their data. Package publication still needs the user's
repository, project-data license and production signing decisions.

## Attested frequency-list experiment

The alternative candidate is [wordfreq](https://github.com/rspeer/wordfreq), pinned
to `42233e6c36ce792031bcccfa17cdd0cec9af5fa7` (upstream v3.2 tag).
Its data describes usage through approximately 2021, not current or exhaustive
spelling coverage. Its [NOTICE](https://github.com/rspeer/wordfreq/blob/master/NOTICE.md)
states separate CC-BY-SA-4.0 data terms and attribution requirements; the Apache
code license alone is insufficient. Keep the complete notice with derivatives.

`tools/fetch_candidate_dictionaries.py NEW_DIRECTORY wordfreq` fetches the full
upstream large lists for AR/BN/DE/ES/FR/JA/PT/ZH and small lists for HI/ID/UR
(no large lists are provided for those three in this snapshot), plus the
readme/license/notice. This is developer source acquisition, not an installer
bulk download. Runtime selection/download behavior is unchanged.

These are upstream-defined finite frequency lists, not affix-expanded Hunspell
dictionaries. Source frequency cutoffs must remain explicit. No additional
top-N cutoff or truncation to the application's byte quota is authorized by this
experiment. Over-limit datasets must fail and trigger a deliberate format/limit
decision. Measure whole-list sizes, normalization losses and script coverage
before adopting the data. Frequency evidence is not proof that a misspelling is
correct, and absence must not itself justify conversion of a valid novel word.

An advisory review suggested corpus filtering but also quota-based truncation;
the latter was not adopted. No runtime morphology engine or semantic change is
approved by that advisory output. Both fixed download allowlists and failure
paths have three offline unit tests (including subcases), run with:

```sh
python3 -B -m unittest discover -s tools -p test_fetch_candidate_dictionaries.py -v
```

### Whole-list audit, 2026-09-09

`python3 -B tools/audit_wordfreq.py target/wordfreq-candidates-20260909`
completed successfully. It validates the fetched inventory and SHA-256, bounds
gzip expansion, and parses only the cBpack v1 schema. It does not execute
upstream code, filter tokens, normalize data, or create installable packages.

| Language | All records (all unique) | Decompressed bytes |
| --- | ---: | ---: |
| AR | 620701 | 8075307 |
| BN | 238743 | 5418875 |
| DE | 634502 | 7387316 |
| ES | 342072 | 3108155 |
| FR | 311419 | 2748516 |
| JA | 214960 | 2382944 |
| PT | 267979 | 2369753 |
| ZH | 334609 | 2844697 |
| HI | 26653 | 448962 |
| ID | 31188 | 246254 |
| UR | 23201 | 250352 |

The large lists have 800 frequency bins; the small lists have 600. These are
upstream boundaries, not newly imposed application cutoffs. No empty tokens
were found. DE has two tokens exceeding the current 64-character/256-byte
limit and one containing whitespace/control; FR has eight containing
whitespace/control. These counts can overlap. Non-NFC records: DE 3, FR 3,
PT 4. Other lists have none of these exceptions. Combining marks are common
in BN (213218 records) and HI (23097); alphabetic-only filtering is unsuitable.
These observations are format checks, not validation of linguistic quality.
No exception has been silently removed or folded into another token.

The strict schema decoder has four passing offline unit tests, including
truncation at every byte, supported array/string encodings, Unicode, bad
headers, oversized declarations, nested invalid values, and trailing data:

```sh
python3 -B -m unittest discover -s tools -p test_audit_wordfreq.py
```

Pending before adoption: explicit treatment of out-of-format records,
full data-license notices in derived artifacts, build/runtime representation
and quota measurements, and language-specific behavioral acceptance.

The audit tool's optional second argument writes a **new** lossless developer
intermediate directory containing every frequency bin and original token plus
the upstream notice/license/readme. It refuses overwrites and emits its
completion marker only after all source integrity checks succeed. This export
is not an installable package. Export, failure-marker and overwrite-preservation
coverage bring its offline test count to five.

`examples/audit_wordfreq_dictionaries.rs` consumes this intermediate and calls
the production `DictionaryPack::from_words` API, then checks membership of every
original token. A rejected complete list is reported without a filtered retry;
the audit finishing does not mean all candidates were accepted. No input or
scoring descriptor is synthesized. `cargo check` and strict Clippy for the
example, formatting, and diff checks pass.

The original full-corpus runtime audit completed in 19.04 seconds. Nine lists
compiled and every original token was found. DE and FR failed strictly, as
expected from the source audit; no filtered retry was used. The two long DE
entries are genuine compound-word forms, not oversized allocations. All nine
whitespace exceptions are a final U+202F (narrow no-break space): DE `00`, and
FR `0000`, `00`, `faire`, `non`, `monde`, `ça`, `000`, `là`, each followed by
that character in the source.

### Prepared dictionary sources

`tools/prepare_wordfreq_sources.py FETCHED_DIRECTORY NEW_SOURCE_DIRECTORY CC_BY_SA_LEGALCODE_FILE`
now creates complete dictionary-source drafts. The only preparation edits are
removing the final U+202F from those nine explicitly allowlisted records.
Unexpected whitespace/control or oversized records abort preparation; there is
no general trimming, script filter, top-N cutoff, or silent row deletion.
Each `SOURCE.json` records original and normalized forms, frequency bin, input
and output counts, unique output count, and file hashes. Duplicate resulting
rows remain present. Original compressed frequency data and complete upstream
notice/readme/license files are retained. These are not installable packs:
short-word policy, scoring and input rules, licenses for project-owned work,
packaging and physical acceptance are still pending.

Preparation completed for all 11 lists in
`target/wordfreq-source-draft-20260909`, retaining every input record. Two
preparation tests cover exact normalization, long-word preservation, duplicates,
source receipts, original bytes/notices, refused overwrites and failed integrity.
Together with the decoder/export tests, seven Python tests pass.

The runtime dictionary compiler now retains words up to its existing 256 UTF-8
byte bound without a separate 64-scalar storage limit. Aggregate byte/row quotas,
whitespace/control rejection and lowercase accounting remain unchanged.
The independent `InputSession` limit is still 64 characters. New tests verify
long compounds, byte-boundary acceptance/rejection, absent adapter readiness,
and clearing/suppression at the 65th input character until a boundary.
Linux library validation: 237 passed, one explicitly ignored full-RU corpus test.

`examples/audit_prepared_wordfreq.rs` validates prepared file receipts and row
counts, compiles each entire list through the production API and checks every
row for membership. Strict Clippy passes. Full prepared-corpus execution passed
for all 11 languages in 27.047 seconds on 2026-09-09 (Longrun job
`c8c7968347014b8ab363cc375629152b`): every input row was found after compilation.
This verifies dictionary storage, not correction readiness or installable packs.
Windows release compilation (all targets) and strict Clippy also passed in job
`12db10045ab04e43869a70be9e8f415c` (82.079 seconds). This was cross-compilation;
the new tests have not yet run on Windows.

### Standalone source attribution

Each newly prepared language directory now contains its own unchanged upstream
NOTICE.md, README.md and Apache LICENSE.txt, the complete CC-BY-SA-4.0 text,
and an ATTRIBUTION.txt that identifies the source revision and local changes.
SOURCE.json hashes every payload, including those notices. It no longer relies
on the parent directory to carry the dictionary's license and attribution.
The preparation tool verifies all notices before creating derivatives and pins
the complete legal text to SHA-256
`28a9529c7d0bb4dc51f4bf5c116a3d16ef247a052f7591466768ddf563fd1cf5`
(20,138 bytes from the [official legal text](https://creativecommons.org/licenses/by-sa/4.0/legalcode.txt)).
Fetch that one public file with:

```sh
python3 -B tools/fetch_candidate_dictionaries.py NEW_LICENSE_DIRECTORY cc-by-sa
```

Standalone preparation completed for all 11 languages without dropping records
under `target/wordfreq-standalone-sources-20260909`. Offline tests cover local
notice copies, every payload receipt, corrupt notices, wrong/oversized legal
text, refused overwrites and retained incomplete output. The full Python suite
has ten passing tests. This does not select a license for project-owned rules,
grant adapter capabilities, create signed packages or publish release assets.

The same preparation was executed into the separate pack project at
`../AutoKeyboardLayot-language-packs/input-sources/wordfreq`.
Recursive comparison confirmed both outputs byte-identical. An independent
check verified all 77 payload receipts and that all 3,046,027 dictionary rows
are byte-identical to the previous runtime-tested prepared lists. The release
index still advertises no input packs; these are source artifacts only.
