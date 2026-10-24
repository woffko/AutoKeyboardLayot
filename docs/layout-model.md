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
  their text; a word counts as its target language only once its conversion
  was applied, and as typed again once that conversion is undone; a token
  kept only for Pause, such as digits or text after a shortcut, does not
  count; the list is cleared when the focused window or field changes).

A small neural network (about one million parameters, 1.3 MB) scores every
reading and a softmax gives the probability of each layout. The word is
converted only when all of the following hold:

1. the dictionary detector did not convert it;
2. it has at least three characters (one- and two-letter words keep the
   dictionary detector's list-based policy);
3. it is not excluded and is not a known word of the typed layout (main
   dictionary, short-word list or user dictionary);
4. it has no character outside the typed language's alphabet at either edge
   and is not a contraction: an apostrophe after a word the typed language
   knows (`don't`, `I've`), or a single apostrophe after three or more
   characters and before one letter (`hadn't`, `Inno's`); nor is it shaped like
   code: such a character after three or more letters that make a word the
   typed language knows (`key.md`, `items[key`, `Default::default`), or a
   period between runs of two or more letters (`ui.py`, `IN.txt`);
5. the target reading uses the target language's alphabet and reads no key
   outside the typed alphabet as a capital letter, which inside a word comes
   from Shift between letters (`LANGID:KLID` would read as `LANGIDÖKLID`);
6. the model is at least 90% sure of another layout (97% for three-letter
   words);
7. as for the dictionary detector, the typed and the target layout are both
   loaded in Windows, each for one language package: without a profile snapshot,
   or with a layout missing or claimed twice, the model sees no other reading. A
   reading the platform does not give falls back to the offline key table.

Typed text is processed in memory only: it is never written to disk, logged or
sent anywhere. Diagnostics record only `stage=model` or `stage=dictionary` next
to the existing candidate counters, never the word, its characters or key
codes.

## Measured effect

Measured with `tools/layout_model` on generated typing: sentences mix the
three languages, and a word is typed in the wrong layout when the user
switches language but forgets to switch the layout. "Realistic" samples words
by real frequency; "rare words" samples each vocabulary uniformly. Words used
only for testing are held out from training. Both detectors use the bundled
English, Russian and Estonian dictionaries. The table was measured with
single-letter words converted ("Automatically convert single-letter words",
`evaluate_layout_detection --single-letter-words`), which is off by default;
the default settings follow it.

| | Dictionary detector | With layout model |
|---|---|---|
| Realistic: wrong-layout words corrected | 86.4% | 95.2% |
| — English / Russian / Estonian | 91.8% / 94.8% / 59.5% | 95.3% / 97.1% / 90.5% |
| Realistic: correct words changed by mistake | 0.09% | 0.10% |
| Rare words: wrong-layout words corrected | 65.8% | 90.4% |
| Rare words: correct words changed by mistake | 0.06% | 0.10% |

With the default settings (single-letter words left as typed) the realistic
sample gives 78.6% and 87.4% of the wrong-layout words corrected (English /
Russian / Estonian 88.1% / 82.0% / 59.4% and 91.6% / 84.3% / 90.4%) and 0.02%
and 0.04% of the correct words changed; the rare-word rates do not change.
Measured on 2026-10-19 with the shipped model and the September test sets,
after the rules for code-shaped tokens (rules 4 and 5). Before them the model
corrected 95.26% of the realistic words (Estonian 90.58%), 87.45% with the
default settings and 90.61% of the rare ones; the generated typing has no
code, so here the rules only cost (0.04 points realistic, 0.20 points rare).
Rule 7 (2026-10-22) leaves every output of these measurements byte for byte the
same: the test cases give the reading of every layout and load all three.

Before rule 4 and the dictionary veto on punctuation after a known word (see
[Limits](#limits)), the dictionary detector corrected 86.5% and the model
96.2% (Estonian 93.3%) of the realistic words and 92.4% of the rare ones. The
generated typing has no punctuation, so here both only cost: words that start
or end with a key that is a letter in the other layout only, such as `ü` or
`б`, no longer reach the model. Their gain is in real text with punctuation.

On real English, Russian, Estonian and foreign (German, French, Spanish)
words the rate of unwanted changes is the same as the dictionary detector's;
the small increase comes from random letter strings in the test data. These
are synthetic measurements, not a substitute for real typing acceptance; see
[Limits](#limits) for text that is not a sentence.

## Limits

The model is a statistical second opinion, not a language identifier. Its limits
are known. The first five are covered by tests in `src/detector.rs`, the
numbers come from `examples/measure_token_false_positives.rs`, and
`tests/dev_tokens.rs` stops the real-token numbers from growing unnoticed.

**The six rules above are hard gates.** A confident model cannot override
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
in two languages or in none that the dictionary stage may use. The model may
decide from the languages of the previous words (never their text), but it
still needs its threshold. In the tests the keys `h]i` read as the Estonian
word `hõi` and as no Russian word, and the dictionary stage leaves
three-letter words to the short-word lists. After three Estonian words the
model is 98% sure of Estonian and converts; after an English word it is 94%
sure and after three Russian words 56% sure, below the 97% a three-letter word
needs, so nothing is converted. Keys whose letter patterns are decisive on
their own (`v'he`, the Estonian `vähe`) convert in every context. The context
is cleared when the focused window or field changes, so the first word typed
there has none.

**Punctuation next to a word is left to the dictionaries.** The model reads
every key as a letter of some layout: on its own it turned `don't` into the
Estonian-looking `donät`, `it.` into the Russian `шею`, a quoted `"Tallinn`
into `ÄTallinn` and the file name `.png` into `юзтп`. Words with a character
outside the typed alphabet at an edge, and contractions, therefore never reach
the model (rule 4), and the dictionary stage ignores punctuation typed after a
word it knows: `key.` stays English although `луню` is a Russian word. Of the
punctuated English words in `tests/fixtures/punctuated-english.txt` neither
stage converts any. The cost is paid by words that start or end with
a key that is a letter only in the other layout: they are corrected only when
the dictionaries know them, and `'kke` stays unconverted after Estonian words
although the model is 91% sure of `äkke` there. Three-letter words are left to
the short-word lists, which since 2026-10-19 hold the commonest of these: the
Russian `был` and `мою` and the Estonian `üle`, `ühe`, `ära` and `töö` (`ühe`
only from the Russian layout: in the English one `[he` stays the English `he`).
With them the realistic sample's corrections rise from 86.4% to 86.9% for the
dictionary detector and from 95.2% to 95.5% with the model, with no more
correct words changed (measured on the build's own dictionaries with
`evaluate_layout_detection --bundled`). Installed packages get them when the
Russian and Estonian packages are published again; the table above was
measured on the published ones.

**Code is left to the dictionaries too.** Inside a token the model read the
colons of `Default::default` and `std::fs` as the Estonian `Ö` and turned
`key.md` into the Russian `лунюьв`: of the 5 826 code paths, file names and
identifiers of `tests/fixtures/technical-tokens.txt` it converted 1 151 (1 145
into Estonian, 1 141 of them with `::`). Rules 4 and 5 stop all of them. Their
cost is paid by words whose letter on a punctuation key follows letters that
spell a word of the typed language, or that have the Russian `ю` between runs
of two or more letters, when the dictionaries do not know them: the Russian
`утюги` (`en.ub`) is converted by the dictionary, an unknown form of that shape
no longer by the model.

**Synthetic and real numbers differ.** The table above measures generated
sentences of real words and shows about 0.1% unwanted changes. Text that is not
a sentence behaves differently. Every row is typed correctly in the English
layout and judged as a first word (no context):

| Input | Dictionary stage converts | Model adds |
|---|---|---|
| 1 659 command, tool and package names | 0.36% | 0.06% |
| 3 016 English words with punctuation next to them | 0% | 0% |
| 5 826 code paths, file names and identifiers | 0% | 0% |
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
the first row with the same command without arguments, the second with
`--punctuated` and the third with `--technical`.

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

   The script is read-only; it never loads, activates or removes layouts. Each
   layout carries its KLID, which tells layouts of one language apart (US
   `00000409` and US-International `00020409` are both language `0409`).

2. **Language entry.** Add an object to `languages` in
   `tools/layout_model/languages.json`:

   - `id`: the input pack ID (for example `de-DE`);
   - `language_id`: the Windows layout language as exported in step 1 (for
     example `0407`);
   - `layout`: the exact layout's KLID as exported in step 1 (for example
     `00000407`); the tools stop when two exported layouts could be meant;
   - `frequency`: `wordfreq:de` for a language covered by
     [wordfreq](https://github.com/rspeer/wordfreq), or `file:NAME` for a
     `word count` list (most frequent first) placed in
     `target/layout-model/sources/`, for example from
     [FrequencyWords](https://github.com/hermitdave/FrequencyWords); a
     `file:` source also needs `sha256` (of the reviewed file) and `source`
     (how the notice names it, with its license);
   - `dictionary`: the main and short word lists, relative to
     `target/layout-model/dicts/` (the same lists as the language's input
     pack);
   - `share`: how often generated sentences use the language as their main
     language.

   Languages written with an IME (Japanese, Chinese) are not supported: their
   text is composed, not produced by one key per character.

   `wordfreq` at the top of the file is the wordfreq version every `wordfreq:`
   source and the noise come from (3.1.1 for the shipped model); install that
   version in step 3, or change it on purpose.

3. **Python environment.**

   ```sh
   uv venv target/layout-model/.venv --python 3.12
   VIRTUAL_ENV=target/layout-model/.venv uv pip install torch numpy wordfreq==3.1.1
   ```

4. **Baseline store (optional).** Copy an installed package store to
   `target/layout-model/store-copy/` (never use the live one) to compare the
   new model with the dictionary detector.

5. **Train, export and evaluate.**

   ```sh
   tools/layout_model/pipeline.sh
   ```

   It checks the inputs (the installed wordfreq version, each `file:` source's
   SHA-256, one exported layout per language), prepares vocabularies (a
   language without words stops it) and records them with their inputs in
   `target/layout-model/vocab/provenance.json`, writes
   `target/layout-model/NOTICE.md` from that receipt, generates 4 million
   training cases and two test sets (any failed generator stops it), trains,
   exports `target/layout-model/model.aklm`, checks the
   quantized export against a reference implementation and, with a store copy,
   prints the comparison tables. Accept a model only if the unwanted-change
   rate on real words (all `kind:` rows except `random`) stays at the
   dictionary detector's level.

   Training is seeded (`train.py fit --seed`, 1 by default) and uses torch's
   deterministic algorithms, and `model.pt.json` and `model.aklm.json` record
   the seed, the Python, torch and numpy versions, the device and the SHA-256
   of the cases, of `languages.json` and of the model. The same inputs, seed,
   versions and device give the same model byte for byte (CPU and GPU may
   differ from each other). The shipped `en-ru-et.aklm` was trained before
   seeding, so it can be retrained to the same quality but not rebuilt byte
   for byte. `tools/test_layout_model_training.py` checks the seeding with
   the environment of step 3; `tools/verify-wsl.sh` runs it with that
   environment, on the CPU only, whenever `target/layout-model/.venv` exists.

6. **Build it in.** Copy `target/layout-model/model.aklm` over
   `data/layout-model/en-ru-et.aklm` and `target/layout-model/fixture.json`
   over `tests/fixtures/layout-model-fixture.json`, run `cargo test`, and build
   the application as usual. Copy `target/layout-model/NOTICE.md` over
   `data/layout-model/NOTICE.md`: it names the sources of that model, with
   their versions and hashes and the credits wordfreq requires for SUBTLEX.

## Files

| Path | Purpose |
|---|---|
| `src/layout_model.rs` | Model format parser and inference in the agent |
| `src/detector.rs` | Second-stage decision rules |
| `data/layout-model/en-ru-et.aklm` | Shipped EN/RU/ET model |
| `tools/layout_model/languages.json` | Languages of a model |
| `tools/layout_model/export_layouts.ps1` | Read-only Windows layout export |
| `tools/layout_model/prepare_vocab.py` | Input checks, frequency vocabularies and their provenance receipt |
| `tools/layout_model/notice.py` | The model's NOTICE, written from that receipt |
| `tools/test_layout_model.py`, `tools/test_layout_model_training.py` | Tests of the tools; the second needs the training environment |
| `tools/layout_model/generate_cases.py` | Synthetic typing cases |
| `tools/layout_model/train.py` | Features, training, prediction |
| `tools/layout_model/export.py` | Binary export and reference check |
| `tools/layout_model/score.py`, `combine.py` | Evaluation |
| `examples/evaluate_layout_detection.rs` | Runs the agent's detector on test cases |
| `examples/measure_token_false_positives.rs` | Counts unwanted conversions on developer tokens or random strings |
| `tests/dev_tokens.rs`, `tests/fixtures/dev-tokens.txt` | Budget for those conversions on 1 659 real tokens |
| `tests/fixtures/punctuated-english.txt` | The same for 3 016 English words with punctuation |
| `tests/fixtures/technical-tokens.txt` | The same for 5 826 code paths, file names and identifiers |
| `tools/layout_model/pipeline.sh` | All steps in order |
