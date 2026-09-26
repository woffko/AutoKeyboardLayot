"""Generate synthetic typing cases for layout-model training and evaluation.

Offline research tool. Nothing here is shipped or read by the agent.

Usage: generate_cases.py LAYOUTS_JSON VOCAB_DIRECTORY OUTPUT.jsonl COUNT SPLIT SEED
Languages, their layouts and sentence shares come from languages.json.
SPLIT is "train" (held-out words excluded), "test" (true word frequencies) or
"uniform" (uniform over each vocabulary, stresses rare words).

Typing model: a sentence has a main language and occasionally switches
language. The keyboard has a current layout. When the intended language needs
another layout, the user either switches first or forgets and types the word
in the current layout. After a word, the layout is the one its final text is
in (the agent converts and switches). A few words are typed in a wrong layout
at random, and some tokens are foreign words or random letters that must stay
unchanged.

Each line: {"id", "typed", "candidates": {layout: text | null}, "label",
"context": [layouts of previous words], "kind", "unseen", "intended"}.
"label" is the layout whose text equals the intended text; it equals "typed"
when the typed text is already correct.
"""
import json
import sys
import zlib
from pathlib import Path

import numpy as np

from common import key_maps, layout_ids, load_config

SWITCH_PROBABILITY = 0.12
SWITCH_FIRST_PROBABILITY = 0.5
RANDOM_WRONG_PROBABILITY = 0.03
NOISE_PROBABILITY = 0.06
RANDOM_LETTERS_PROBABILITY = 0.01


def held_out(word):
    return zlib.crc32(word.encode("utf-8")) % 10 == 0


def load_vocab(path, split):
    words, weights = [], []
    with open(path, encoding="utf-8") as stream:
        for line in stream:
            word, frequency = line.rstrip("\n").split("\t")
            if split == "train" and held_out(word):
                continue
            words.append(word)
            weights.append(float(frequency))
    weights = np.asarray(weights)
    if split == "train":
        weights = weights**0.7
    elif split == "uniform":
        weights = np.ones_like(weights)
    return words, weights / weights.sum()


class Sampler:
    def __init__(self, rng, words, weights, batch=65536):
        self.rng, self.words, self.weights, self.batch = rng, words, weights, batch
        self.buffer = []

    def __call__(self):
        if not self.buffer:
            self.buffer = list(self.rng.choice(len(self.words), self.batch, p=self.weights))
        return self.words[self.buffer.pop()]


def apply_case(rng, word):
    roll = rng.random()
    if roll < 0.08:
        return word[:1].upper() + word[1:]
    if roll < 0.09 and len(word) > 1:
        return word.upper()
    return word


def type_word(text, intended_layout, typed_layout, to_key, to_char, layouts):
    """Keys of `text` on the intended layout, read back on every layout."""
    keys = []
    for character in text:
        key = to_key[intended_layout].get(character)
        if key is None:
            return None
        keys.append(key)
    candidates = {}
    for layout in layouts:
        mapped = [to_char[layout].get(key) for key in keys]
        candidates[layout] = None if None in mapped else "".join(mapped)
    if candidates[typed_layout] is None:
        return None
    return candidates


def main():
    layouts_path, vocab_dir, output, count, split, seed = sys.argv[1:7]
    count, rng = int(count), np.random.default_rng(int(seed))
    config = load_config()
    layout_names = layout_ids(config)
    to_key, to_char = key_maps(config, layouts_path)
    vocab_dir = Path(vocab_dir)
    samplers = {
        code: Sampler(rng, *load_vocab(vocab_dir / f"{code}.tsv", split))
        for code in layout_names + ["noise"]
    }
    letters = {
        layout: sorted(c for c in set(to_char[layout].values()) if c.isalpha() and c.islower())
        for layout in layout_names
    }
    main_codes = layout_names
    main_weights = np.array([language["share"] for language in config["languages"]], dtype=float)
    main_weights /= main_weights.sum()
    noise_layout = config["noise_layout"]
    written = 0
    with open(output, "w", encoding="utf-8") as stream:
        while written < count:
            main = rng.choice(main_codes, p=main_weights)
            current_layout = rng.choice(layout_names)
            context = []
            language = main
            for _ in range(int(rng.integers(3, 16))):
                if rng.random() < SWITCH_PROBABILITY:
                    language = rng.choice([c for c in main_codes if c != language])
                elif language != main and rng.random() < 0.4:
                    language = main
                roll = rng.random()
                if roll < RANDOM_LETTERS_PROBABILITY:
                    kind = "random"
                    typed_layout = current_layout
                    length = int(rng.integers(2, 8))
                    text = "".join(rng.choice(letters[typed_layout], length))
                    intended_layout = typed_layout
                elif roll < RANDOM_LETTERS_PROBABILITY + NOISE_PROBABILITY:
                    kind = "noise"
                    text = apply_case(rng, samplers["noise"]())
                    intended_layout = typed_layout = noise_layout
                else:
                    kind = str(language)
                    text = apply_case(rng, samplers[language]())
                    intended_layout = str(language)
                    if intended_layout != current_layout and rng.random() < SWITCH_FIRST_PROBABILITY:
                        current_layout = intended_layout
                    typed_layout = current_layout
                    if rng.random() < RANDOM_WRONG_PROBABILITY:
                        typed_layout = rng.choice([l for l in layout_names if l != intended_layout])
                candidates = type_word(text, intended_layout, typed_layout, to_key, to_char, layout_names)
                if candidates is None:
                    continue
                if kind in ("noise", "random"):
                    # Unchanged text is the only correct outcome.
                    candidates = type_word(text, typed_layout, typed_layout, to_key, to_char, layout_names)
                    if candidates is None:
                        continue
                    label = typed_layout
                else:
                    label = typed_layout if candidates[typed_layout] == text else intended_layout
                stream.write(
                    json.dumps(
                        {
                            "id": written,
                            "typed": typed_layout,
                            "candidates": candidates,
                            "label": label,
                            "context": context[-4:],
                            "kind": kind,
                            "unseen": kind in layout_names and held_out(text.lower()),
                            "intended": text,
                        },
                        ensure_ascii=False,
                    )
                    + "\n"
                )
                written += 1
                context.append(label)
                current_layout = label
                if written >= count:
                    break
    print(f"{output}: {written} cases")


if __name__ == "__main__":
    main()
