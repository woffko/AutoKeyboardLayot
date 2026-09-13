# Offline candidate-model training

`cargo run --offline --example train_wordfreq_models -- PREPARED_ROOT NEW_OUTPUT_DIRECTORY`
consumes the eleven prepared wordfreq dictionaries. It validates each words-file
hash, byte count and row count against its preparation receipt before training.
The output directory must not exist. A failed run can leave partial experimental
output; it never installs a package or overwrites a previous experiment.

The generator scans every dictionary row, selecting lowercase alphabetic tokens
in the language's designated scripts for **training only**. Inherited combining
marks are allowed after a base; other marks require a matching Script_Extensions
entry. This is a candidate repertoire heuristic, not linguistic validation or
composition handling. Both selected and excluded row counts are reported. The
original dictionary is untouched, including records unsuitable for this model.

Observed letters are represented by a format-4 explicit list without filling
gaps or truncating the repertoire. The report also counts the contiguous ranges
that an older format would require. Combining marks are separate. Bigrams and
trigrams use distinct-ngram row support, with deterministic lexical tie breaking
and a 512-feature limit each. This is not corpus-frequency weighting. Vowel
heuristics and unknown-word statistical targets remain disabled pending
language-specific calibration. The runtime parser checks every generated model;
its rejection is recorded, not bypassed by widening runtime limits automatically.
For an accepted model, every selected training token is checked again through
the runtime token validator; any membership loss aborts the run.

The `.candidate.json` and `.training.json` files are experimental derivatives,
not distributable standalone packs. Distribution still requires the original
prepared source notices, licenses and attribution, separately reviewed short-word
tiers, input profiles and the physical acceptance gates. Script classification
uses pinned developer-only `unicode-script` 0.5.8; mark categories use the same
`unicode-general-category` 1.1.0 as the runtime.

## Full-source verification, 2026-09-09

Longrun `385561c61447454ba5fd3a443d450a1e` completed in 175.153 seconds.
All eleven format-4 candidates passed the runtime parser and every selected
training token passed `accepts_word`. Outputs and source hashes are recorded in
`target/wordfreq-candidate-models-v4-20260909/*.training.json`.

| Language | Source rows | Training rows | Observed letters |
| --- | ---: | ---: | ---: |
| AR | 620701 | 583561 | 80 |
| BN | 238743 | 213607 | 51 |
| DE | 634502 | 626849 | 127 |
| ES | 342072 | 336283 | 121 |
| FR | 311419 | 304298 | 123 |
| JA | 214960 | 165800 | 5396 |
| PT | 267979 | 262195 | 115 |
| ZH | 334609 | 297120 | 9205 |
| HI | 26653 | 23889 | 49 |
| ID | 31188 | 30749 | 34 |
| UR | 23201 | 22422 | 54 |

No dictionary records were removed. This verifies model representation and
training-token membership, not linguistic accuracy, short-word calibration,
input-profile compatibility or physical typing. The preceding range-based
experiment rejected DE/ES/FR/JA/ZH because of representation bounds; format 4
preserves their exact observed letter sets within the existing 64 KiB file cap.
