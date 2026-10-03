# Layout model (experimental)

AutoKeyboardLayot detects a word typed in the wrong keyboard layout with a
dictionary detector: the same physical keys are read on every enabled layout,
and a reading that is a known word of another language replaces the typed
text. That detector is precise, but it misses words that are not in its
dictionaries, especially inflected forms. The Estonian dictionary contains
mostly base forms, so many Estonian words were never corrected.

The layout model is an optional second stage for English, Russian and
Estonian. It runs only when the dictionary detector leaves a word unchanged.

## How it decides

For every layout reading of the typed keys the model combines:

- character n-grams (1 to 4 letters) of that reading, learned per language;
- whether the reading is in the language's dictionary or short-word list;
- whether it is the typed layout and whether it equals the typed text;
- the languages of up to four previous words (only their language, never
  their text, and cleared when the focused window or field changes).

A small neural network (about one million parameters, 1.3 MB) scores every
reading and a softmax gives the probability of each layout. The word is
converted only when all of the following hold:

1. the dictionary detector did not convert it;
2. it has at least three characters (one- and two-letter words keep the
   dictionary detector's list-based policy);
3. it is not excluded and is not a known word of the typed layout (main
   dictionary, short-word list or user dictionary);
4. the target reading uses the target language's alphabet;
5. the model is at least 90% sure of another layout (97% for three-letter
   words).

Typed text is processed in memory only: it is never written to disk, logged or
sent anywhere. Diagnostics record only `stage=model` or `stage=dictionary` next
to the existing candidate counters, never the word, its characters or key
codes.

## Measured effect

Measured with `tools/layout_model` on generated typing: sentences mix the
three languages, and a word is typed in the wrong layout when the user
switches language but forgets to switch the layout. "Realistic" samples words
by real frequency; "rare words" samples each vocabulary uniformly. Words used
only for testing are held out from training. Both detectors use the installed
signed packages.

| | Dictionary detector | With layout model |
|---|---|---|
| Realistic: wrong-layout words corrected | 86.5% | 96.2% |
| — English / Russian / Estonian | 91.8% / 94.8% / 59.8% | 95.3% / 97.8% / 93.3% |
| Realistic: correct words changed by mistake | 0.09% | 0.11% |
| Rare words: wrong-layout words corrected | 65.8% | 92.4% |
| Rare words: correct words changed by mistake | 0.06% | 0.10% |

On real English, Russian, Estonian and foreign (German, French, Spanish)
words the rate of unwanted changes is the same as the dictionary detector's;
the small increase comes from random letter strings in the test data. These
are synthetic measurements, not a substitute for real typing acceptance; see
[Limits](#limits) for text that is not a sentence.

## Limits

The model is a statistical second opinion, not a language identifier. Its limits
are known. The first three are covered by tests in `src/detector.rs`, the
numbers come from `examples/measure_token_false_positives.rs`, and
`tests/dev_tokens.rs` stops the real-token numbers from growing unnoticed.

**The five rules above are hard gates.** A confident model cannot override
them. `kerf` is an English dictionary word whose keys also read as the common
Russian word `лука`; the model alone is 99% sure of Russian and rule 3 keeps
the word English. The protection is only as good as the dictionary: a name, a
command or a rare word that the English lists do not contain has none.

**The thresholds are exact.** Two Russian dictionary words of three letters
that the dictionary stage leaves alone are not converted when the model is 94%
and 96% sure, and one is converted at 99.8%. A longer word is converted from
90%. One- and two-letter words never reach the model, however sure it is.

**Ambiguity is settled by context, and only above the threshold.** English and
Estonian share the key of every letter, and some key sequences read as a word
in two languages. The dictionary stage then fails closed. The model may decide
from the languages of the previous words (never their text), but it still needs
90%. In the tests the keys `'kke` read as the Russian word `эллу` and as the
Estonian word `äkke`. After three Estonian words the model is 91% sure of
Estonian and converts; after an English word it is 81% sure of Estonian, and
after three Russian words 65% sure of Russian, so nothing is converted. Keys
whose letter patterns are decisive on their own (`v'he`, the Estonian `vähe`)
convert in every context. The context is cleared when the focused window or
field changes, so the first word typed there has none.

**Synthetic and real numbers differ.** The table above measures generated
sentences of real words and shows about 0.1% unwanted changes. Text that is not
a sentence behaves differently. Every row is typed correctly in the English
layout and judged as a first word (no context):

| Input | Dictionary stage converts | Model adds |
|---|---|---|
| 1 659 command, tool and package names | 0.36% | 0.06% |
| Random lowercase letters, 3 letters | 0.36% | 3.3% |
| 4 letters | 1.5% | 2.4% |
| 6 letters | 0.05% | 1.7% |
| 8 letters | 0.5% | 0.75% |
| 12 letters | 4.6% | 0.17% |
| 16 letters | 3.3% | 0.05% |

The random rows are a worst case (40 000 uniformly random strings per row), not
typical typing. They show what no dictionary protects: machine-generated
lowercase text. Reproduce a row with
`cargo run --release --example measure_token_false_positives -- --random 6 40000`,
and the first row with the same command without arguments.

**False-positive classes** seen in these measurements:

- Short names that read as a common word of the other language: `dtc` → `вес`
  and `dtls` → `веды`, and three-letter abbreviations that the general English
  list holds but the short-word list does not (`tot` → `еще`, `dbl` → `вид`).
- Estonian words typed in the English layout (`endi`, `lebe`, `muda`). Letters
  share their keys in both layouts, so the Estonian reading is the typed text
  and the conversion changes no letter.
- Long unknown letter strings: `nfsdclnts` → `таывсдтеы`. The dictionary stage
  accepts Cyrillic text that merely looks pronounceable ("statistical targets"),
  which is why random strings of ten letters or more are converted in 3 to 5% of
  cases.
- Short unknown strings once the model is on: `gtk` → `пел`.

**What to do about it.** The model is off by default. In a terminal, add the
terminal's executable to the program exclusions in Settings, or list the
commands you type often in the word exclusions (`en-US: kubectl`). This page
documents the behavior; it does not change the detection policy. Lowering a
class of false positives is a deliberate decision with its own measurement:
change the budgets in `tests/dev_tokens.rs` only together with that decision.

## Enabling it

The model is off by default. Select **Settings → General → Experimental
layout model for EN/RU/ET words missing from the dictionaries** and then
Apply; clear the checkbox and Apply to return to the dictionary detector
only. The change takes effect immediately, without restarting.

The same switch is the `layout_model=true|false` line under `[settings]` in
`%LOCALAPPDATA%\AutoKeyboardLayot\config.ini`. Installed interface language
packages published before this option show its label in English until they
are updated.

It needs the English, Russian and Estonian input packs; other enabled
languages keep using the dictionary detector only.

## Adding other languages

The tools are generic: a model is trained for the set of languages listed in
`tools/layout_model/languages.json` (2 to 16 languages). Adding a language
currently means training a new model and building the application with it.
Shipping models inside signed language packages is planned but not done.

Requirements: Windows with every model language's keyboard layout installed,
WSL or Linux with Rust and Python 3.12, and ideally an NVIDIA GPU (training
takes about a minute on a GPU and longer on a CPU).

1. **Keyboard layouts.** Add the layout in Windows settings, then export what
   every physical key produces:

   ```powershell
   powershell -ExecutionPolicy Bypass -File tools\layout_model\export_layouts.ps1 -Output target\layout-model\layouts.json
   ```

   The script is read-only; it never loads, activates or removes layouts.

2. **Language entry.** Add an object to `languages` in
   `tools/layout_model/languages.json`:

   - `id`: the input pack ID (for example `de-DE`);
   - `language_id`: the Windows layout language as exported in step 1 (for
     example `0407`);
   - `frequency`: `wordfreq:de` for a language covered by
     [wordfreq](https://github.com/rspeer/wordfreq), or `file:NAME` for a
     `word count` list (most frequent first) placed in
     `target/layout-model/sources/`, for example from
     [FrequencyWords](https://github.com/hermitdave/FrequencyWords);
   - `dictionary`: the main and short word lists, relative to
     `target/layout-model/dicts/` (the same lists as the language's input
     pack);
   - `share`: how often generated sentences use the language as their main
     language.

   Languages written with an IME (Japanese, Chinese) are not supported: their
   text is composed, not produced by one key per character.

3. **Python environment.**

   ```sh
   uv venv target/layout-model/.venv --python 3.12
   VIRTUAL_ENV=target/layout-model/.venv uv pip install torch numpy wordfreq
   ```

4. **Baseline store (optional).** Copy an installed package store to
   `target/layout-model/store-copy/` (never use the live one) to compare the
   new model with the dictionary detector.

5. **Train, export and evaluate.**

   ```sh
   tools/layout_model/pipeline.sh
   ```

   It prepares vocabularies, generates 4 million training cases and two test
   sets, trains, exports `target/layout-model/model.aklm`, checks the
   quantized export against a reference implementation and, with a store copy,
   prints the comparison tables. Accept a model only if the unwanted-change
   rate on real words (all `kind:` rows except `random`) stays at the
   dictionary detector's level.

6. **Build it in.** Copy `target/layout-model/model.aklm` over
   `data/layout-model/en-ru-et.aklm` and `target/layout-model/fixture.json`
   over `tests/fixtures/layout-model-fixture.json`, run `cargo test`, and build
   the application as usual. Update `data/layout-model/NOTICE.md` with the
   sources of the new frequency data.

## Files

| Path | Purpose |
|---|---|
| `src/layout_model.rs` | Model format parser and inference in the agent |
| `src/detector.rs` | Second-stage decision rules |
| `data/layout-model/en-ru-et.aklm` | Shipped EN/RU/ET model |
| `tools/layout_model/languages.json` | Languages of a model |
| `tools/layout_model/export_layouts.ps1` | Read-only Windows layout export |
| `tools/layout_model/prepare_vocab.py` | Frequency vocabularies |
| `tools/layout_model/generate_cases.py` | Synthetic typing cases |
| `tools/layout_model/train.py` | Features, training, prediction |
| `tools/layout_model/export.py` | Binary export and reference check |
| `tools/layout_model/score.py`, `combine.py` | Evaluation |
| `examples/evaluate_layout_detection.rs` | Runs the agent's detector on test cases |
| `examples/measure_token_false_positives.rs` | Counts unwanted conversions on developer tokens or random strings |
| `tests/dev_tokens.rs`, `tests/fixtures/dev-tokens.txt` | Budget for those conversions on 1 659 real tokens |
| `tools/layout_model/pipeline.sh` | All steps in order |
