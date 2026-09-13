# Composition and text-edit boundaries

The current adapter remains `ConservativePhysicalKeys`, not a composition-aware
IME adapter. Model acceptance, Unicode segmentation, and a target application's
actual deletion behavior are separate checks.

## Verified scalar-count gap

Rust's Unicode Alphabetic predicate accepts U+0345, U+093E and U+09BE, and their
lowercase mapping is unchanged. Consequently, the existing scoring range parser
can accept some combining marks. It does not prove that every accepted sequence
is a series of independently erasable characters. The distinction between derived
Alphabetic and general category is described by
[UAX #44](https://www.unicode.org/reports/tr44/#Property_Definitions).

The current word buffer and physical replay use scalar counts. `ToUnicodeEx`
translation rejects multi-scalar output; replay mapping rejects outputs that are
not one UTF-16 unit. None of these facts establishes grapheme-aware deletion for
every string accepted by a data model.

`ConversionTransaction` now checks both complete edit strings, including the
delimiter, before producing either forward or undo instructions. Each extended
grapheme must contain exactly one scalar and one UTF-16 unit for this existing
Backspace protocol. Composite graphemes and supplementary-plane scalars are
rejected, not assigned an assumed Backspace count. This preserves existing
EN/RU/ET ordinary-key edits. Segmentation uses pinned `unicode-segmentation`
1.13.3 and the extended grapheme rules described by
[UAX #29](https://www.unicode.org/reports/tr29/#Grapheme_Cluster_Boundaries).

This is an edit compatibility guard, not an implementation of IME correction.
It does not claim that all single-scalar graphemes are safe in arbitrary editors;
existing focus, privacy, profile, composition and event checks remain necessary.
Public `TextEdit` construction is not an authorization boundary either.

The edit-unit guard passed Windows release compilation for all targets and
strict Clippy on 2026-09-09 (job `9158260aa2f64886b4ca1b3b1a30d1c4`,
93.093 seconds). This is cross-compilation, not native input acceptance.

## Explicit scoring marks

Scoring format 3 separates base ranges from a bounded, explicit mark repertoire.
General Category Mn/Mc/Me is checked with pinned Unicode 16.0 tables, rather than
inferring marks from the Alphabetic property. This distinction follows the
[Unicode General Category definitions](https://www.unicode.org/reports/tr44/#General_Category_Values).
Accepted marks may continue a scoring token but cannot start it; automatic and
forced target validation share this check. Data may explicitly classify a
dependent vowel sign as a vowel. Scalar scoring units remain unchanged.

This lets language models describe their combining signs without losing them
or mixing them into base-letter ranges. It does not enable a new input profile
or make the existing edit protocol composition-aware. The integration test
scores a decomposed dictionary token successfully, then verifies that the
current transaction constructor refuses its composite-text edit. An initial
mark can still enter the transient character buffer; this is not a claim of
context-aware input segmentation. Per-language models and their acceptance are
still required.

## Remaining adapter work

Required work still includes composition lifecycle handling, language-specific
word boundaries, dead-key/AltGr handling, consistent buffer/backspace semantics,
and a host-validated replacement/undo protocol for complex text. Grapheme count
alone must not be substituted for a target editor's deletion semantics. Physical
acceptance in the planned Windows applications remains required before declaring
new input profiles ready. No capability was added by this change.
