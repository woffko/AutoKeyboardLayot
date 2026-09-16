# Plan: opt-in automatic conversion of single-letter words

Project: AutoKeyboardLayot (`/home/w0w/AutoKeyboardLayot`, branch `main`).
Owner decisions recorded 2026-09-15.

## Decisions

1. **Ship single-letter auto-conversion as an opt-in.** Default off. Delivered
   single-letter targets: `я` (ru-RU) and `a`, `i` (en-US). Every other
   single-letter word is opt-in per user through the user dictionary.
2. **Publish the data.** `ru-RU` revision 3 carries `я` in its short tier, plus a
   new catalog revision and release. The embedded `en-US` short tier ships in the
   binary, so `a`/`i` need no package.
3. **Two-letter false positives: no default curation (variant C).** The
   two-letter tier is left unchanged. A user who dislikes a specific conversion
   suppresses it with a user-dictionary entry (for example `en-US: vs`).
4. The plan is kept in this file.

Rules that still apply: GitHub-bound content is English; no secrets in commits;
never regenerate signing keys; no package/catalog publication without the owner's
explicit consent; no temporary diagnostics left in the code.

## Verified facts (current tree, HEAD `5671504`)

Four gates currently keep single letters out of automatic conversion:

1. `DetectorConfig::minimum_word_characters = 2` (`src/detector.rs`).
2. A word shorter than `MINIMUM_STATISTICAL_CHARACTERS = 4` must be present in
   the target's short tier or the user dictionary; no single-letter rows exist.
3. `build.rs` drops every row shorter than two characters for all embedded
   tiers, including the `*-short` packs (`normalized.chars().count() >= 2`).
   `src/test_support.rs` replicates the same filter. `examples/export_input_sources.rs`
   exports package sources from the compiled `OUT_DIR/*.fst` sets, so the
   installed ru-RU/et-EE packages carry no single-character rows either.
4. Score gate: a single-character target scores at most
   `1 * 1.25 + 10 = 11.25 < minimum_target_score = 12.0`, because there are no
   bigrams/trigrams and the vowel term needs a ratio in `0.15..=0.70`.

Other relevant facts:

- For short words the detector's "known source" test uses only the short tier or
  the user dictionary (`src/detector.rs`, `detect_mapped_candidates`); the base
  dictionary is deliberately ignored for short sources.
- Automatic conversion is created only on a physical Space (`VK_SPACE` arm in
  `windows_agent.rs`). Enter/Tab/punctuation boundaries never produce a
  transaction.
- The privacy exception from `5671504` is forced-only; automatic conversion in a
  console stays blocked by `InspectionUnavailable`.
- The detector is constructed in `windows_agent.rs` at the two
  `Detector::with_profile_selections(Default::default(), ...)` sites
  (`new_with_lexicon_candidate` and the configuration reload). The reload reads
  the flag before `self.settings = configuration.settings`.
- Layout mapping: `src/language.rs` (`ENGLISH_LOWER` / `RUSSIAN_LOWER`).
- Localization: `data/locales/en.json` plus fourteen `data/package-locales/*.json`;
  `tests/package_locale_sources.rs` requires every optional catalog to be
  complete, and `src/localization.rs` scans `settings.slint` for literal keys.

## Slice 0 — this document

Create `docs/plan-single-letter-auto-conversion.md` with this content.

## Slice 1 — detector policy (`src/detector.rs`)

1. Add `pub single_letter_words: bool` to `DetectorConfig` (default `false`).
   Update the struct literals in the detector tests to use `..Default::default()`.
2. In `detect_mapped_candidates`, after `active_language(...)` and the length
   computation: when `character_count == 1`, return `None` if the flag is off,
   otherwise delegate to `detect_single_letter`.
3. Add `detect_single_letter`: list-only policy, no score/margin thresholds.
   The base dictionary tier is ignored on purpose. Fail-closed invariants:
   word exclusions win; a known source (short tier or user dictionary of the
   current language) is never converted; the replacement must be exactly one
   character; an identical candidate is ignored; more than one qualifying target
   returns `None`. `source_score`/`target_score` are filled for diagnostics only.
4. Tests (detector module plus `session.rs::type_word`):
   - default config: `detect("z", English)` and `detect("я", Russian)` are `None`.
   - flag on, `я` in the ru-RU short tier: `z -> я`, `Z -> Я`; `ф -> a`,
     `Ш -> I`, `ш -> i`.
   - flag on, `я` only in the user dictionary: `z -> я` still works.
   - flag on: `я` typed in RU is left alone; `a`, `I` typed in EN are left alone.
   - flag on: `b -> None` until `ru-RU и` is in the user dictionary, then `b -> и`.
   - flag on: word exclusion `en-US z` yields `None`.
   - flag on: an identical candidate (Estonian `z`) is ignored.
   - flag on: two qualifying targets fail closed.
   - `session.rs`: `type_word("z", English)` yields `Candidate` with the flag on
     and `None` with it off.
   - `force_mapped_candidates` is unchanged (regression assertion with the flag
     off).

## Slice 2 — data tiers, build script, docs

1. `build.rs::build_dictionary`: keep `>= 2` for base tiers and allow `>= 1` for
   the short tier (`InputKind::Plain`).
2. `src/test_support.rs`: same rule in the registry normalization closure.
3. Lists: `data/language-packs/ru-RU/common-short-words.txt` gains `я`;
   `data/language-packs/en-US/common-short-words.txt` gains `a` and `i`;
   `data/language-packs/et-EE/common-short-words.txt` is unchanged.
4. Docs: update `data/language-packs/README.md`,
   `docs/language-package-format.md` (short tier is 1-3 characters) and
   `README.md` (new setting, Space-only trigger).

## Slice 2b — two-letter tier (variant C)

No data change. Document in `data/language-packs/README.md` that a specific
unwanted two-letter conversion is suppressed with a user-dictionary entry such as
`en-US: vs`, and add a regression test that `detect("vs", English)` is `None`
once `en-US vs` is in the user dictionary while the plain detector still converts
it.

The two-letter tier currently maps the following Latin tokens into Russian short
words (source `en-US`, target `ru-RU`, base dictionary not used for short
sources):

| source | target | source | target | source | target |
| --- | --- | --- | --- | --- | --- |
| `bkb` | `или` | `bp` | `из` | `bv` | `им` |
| `cfv` | `сам` | `cj` | `со` | `cjy` | `сон` |
| `cnj` | `сто` | `dbl` | `вид` | `dct` | `все` |
| `dfc` | `вас` | `dfv` | `вам` | `dj` | `во` |
| `djn` | `вот` | `djy` | `вон` | `ds` | `вы` |
| `dtc` | `вес` | `dtr` | `век` | `ev` | `ум` |
| `ghb` | `при` | `ghj` | `про` | `gj` | `по` |
| `gjk` | `пол` | `gjl` | `под` | `hfp` | `раз` |
| `jn` | `от` | `jy` | `он` | `jyb` | `они` |
| `jyf` | `она` | `jyj` | `оно` | `kb` | `ли` |
| `kju` | `лог` | `ktn` | `лет` | `ldf` | `два` |
| `ldt` | `две` | `lf` | `да` | `lfv` | `дам` |
| `lj` | `до` | `ljv` | `дом` | `lkz` | `для` |
| `lyb` | `дни` | `lyj` | `дно` | `nen` | `тут` |
| `nfr` | `так` | `nfv` | `там` | `nhb` | `три` |
| `nj` | `то` | `njn` | `тот` | `njv` | `том` |
| `ns` | `ты` | `nt` | `те` | `ntv` | `тем` |
| `pf` | `за` | `pfk` | `зал` | `pkj` | `зло` |
| `rfr` | `как` | `rj` | `ко` | `rnj` | `кто` |
| `tlf` | `еда` | `tot` | `еще` | `tq` | `ей` |
| `tt` | `ее` | `tuj` | `его` | `tve` | `ему` |
| `ujl` | `год` | `ult` | `где` | `vbh` | `мир` |
| `vjq` | `мой` | `vjt` | `мое` | `vjz` | `моя` |
| `vs` | `мы` | `vyt` | `мне` | `xnj` | `что` |
| `yb` | `ни` | `ybv` | `ним` | `ye` | `ну` |
| `yf` | `на` | `yfc` | `нас` | `yfi` | `наш` |
| `yfv` | `нам` | `yj` | `но` | `yt` | `не` |
| `ytn` | `нет` | `ytq` | `ней` |  |  |

None of these is added to the default short tier: they are the common
wrong-layout forms of very frequent Russian words, and suppressing them by
default would remove the feature's main value.

## Slice 3 — setting, configuration plumbing, UI, localization

1. `src/settings.rs`: `pub single_letter_words: bool` (default `false`), parser
   key `single_letter_words`, a `to_text` line after
   `physical_fallback_for_unsupported_apps`, and parse/round-trip tests.
2. `src/windows_agent.rs`: pass
   `DetectorConfig { single_letter_words: configuration.settings.single_letter_words, ..Default::default() }`
   at both construction sites; add a reload test.
3. `src/windows_agent/settings_window.rs`: set/get `single_letter_words` next to
   `recheck_first_word_after_erasing`.
4. `ui/settings.slint`: boolean property plus a checkbox after
   `general.recheck_erased`; verify the content-sized General card does not clip.
5. Localization: `general.single_letter_words` in `data/locales/en.json` and all
   fourteen `data/package-locales/*.json`; keep the `z -> я` example literal.
   Validate with
   `cargo run --locked --offline --example validate_locales -- data/package-locales --require-complete`.

## Slice 4 — publication

1. Build with default features (`--features legacy-bundled-input`) so
   `OUT_DIR/ru-RU-short.fst` includes `я`, then export the source directory with
   `examples/export_input_sources`.
2. Prepare `ru-RU` revision 3 from a recipe that points at the exported words and
   short words, then sign it with the existing key (never regenerate).
3. Build catalog revision 4 from the twelve unchanged packages, `ru-RU-r3` and
   `et-EE-r2`, then sign it.
4. Publish release `lang-r4-20260915` with every `.aklp` asset and `catalog.aklc`;
   verify `releases/latest/download/catalog.aklc`.
5. Install `ru-RU rev3` through the settings "Add language" dialog.

## Slice 5 — verification on the Windows host

Build with `cargo xwin build --release --locked --offline --no-default-features
--target x86_64-pc-windows-msvc --target-dir target/xwin-hotkeys --jobs 2 --bin
AutoKeyboardLayot`, replace the installed executable and restart the agent.

Checks (Notepad, diagnostics on, flag on, ru-RU rev3 installed):

1. EN layout: `z ` -> `я ` and the layout switches to RU; log shows
   `event=candidate forced=false source=en-US target=ru-RU original_chars=1
   replacement_chars=1` and `event=conversion ... result=applied`.
2. EN layout: `Z ` -> `Я `.
3. EN layout: `b `, `c `, `d `, `r ` unchanged (no `candidate` line).
4. EN layout: `b. ` and `b) ` unchanged.
5. RU layout: `ф ` -> `a `, `Ш ` -> `I `; `я `, `и ` unchanged.
6. Pause after case 1 undoes it and offers the word exclusion; accepting it makes
   `z ` stop converting.
7. User-dictionary `ru-RU: и` makes `b ` -> `и `; remove it afterwards.
8. Flag off: case 1 stops converting; Pause still cycles `z <-> я` manually.
9. Windows Terminal: automatic conversion stays blocked; Pause still works.
10. The settings checkbox round-trips through Apply and survives a restart.

Linux/WSL gates before deploying: `cargo test --lib`,
`cargo clippy --target x86_64-pc-windows-msvc --features installer-tools
--all-targets -- -D warnings`, the offline Windows `cargo check`, and the locale
validation from Slice 3.
