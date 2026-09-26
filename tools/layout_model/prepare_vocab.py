"""Prepare frequency-weighted vocabularies for the layout model.

Offline tool. Nothing here is shipped or read by the agent.

Usage: prepare_vocab.py LAYOUTS_JSON SOURCES_DIRECTORY OUTPUT_DIRECTORY

For every language in languages.json the "frequency" entry names the source:
  wordfreq:CODE  the wordfreq package (data CC BY-SA 4.0, see its NOTICE);
  file:NAME      a "word count" list in SOURCES_DIRECTORY, most frequent first
                 (for example FrequencyWords, content CC BY-SA 4.0).
Only lowercase alphabetic words whose every letter exists on the language's
own layout (with or without Shift, excluding dead keys) are kept.
noise.tsv collects words of the "noise" languages that are fully typeable on
noise_layout and unknown to the model languages; they must stay unchanged.
"""
import sys
from pathlib import Path

import wordfreq

from common import load_config, load_layouts

TOP_N = 200_000
NOISE_PER_LANGUAGE = 30_000


def layout_letters(layout):
    letters = set()
    for key in layout["keys"].values():
        for value in (key["normal"], key["shift"]):
            if value and value.isalpha():
                letters.add(value.lower())
    return letters


def keep(word, letters):
    return word.isalpha() and word == word.lower() and all(c in letters for c in word)


def write(path, entries):
    with path.open("w", encoding="utf-8") as stream:
        for word, frequency in entries:
            stream.write(f"{word}\t{frequency:.6g}\n")
    print(f"{path.name}: {len(entries)} words")


def frequencies(source, sources_dir, letters):
    kind, _, name = source.partition(":")
    if kind == "wordfreq":
        return [
            (word, wordfreq.word_frequency(word, name))
            for word in wordfreq.top_n_list(name, TOP_N)
            if keep(word, letters)
        ]
    if kind == "file":
        counts = []
        with (sources_dir / name).open(encoding="utf-8") as stream:
            for line in stream:
                word, _, count = line.rstrip("\n").rpartition(" ")
                if keep(word, letters) and count.isdigit():
                    counts.append((word, int(count)))
                if len(counts) >= TOP_N:
                    break
        total = sum(count for _, count in counts)
        return [(word, count / total) for word, count in counts]
    raise SystemExit(f"unknown frequency source {source}")


def main():
    layouts_path, sources_dir, output = map(Path, sys.argv[1:4])
    config = load_config()
    layouts = load_layouts(layouts_path)
    output.mkdir(parents=True, exist_ok=True)
    known = set()
    for language in config["languages"]:
        letters = layout_letters(layouts[language["language_id"]])
        entries = frequencies(language["frequency"], sources_dir, letters)
        write(output / f"{language['id']}.tsv", entries)
        known.update(word for word, _ in entries)
    noise_language = next(l for l in config["languages"] if l["id"] == config["noise_layout"])
    letters = layout_letters(layouts[noise_language["language_id"]])
    noise = []
    for code in config["noise"]:
        for word in wordfreq.top_n_list(code, NOISE_PER_LANGUAGE):
            if keep(word, letters) and word not in known and len(word) > 1:
                noise.append((word, wordfreq.word_frequency(word, code)))
    write(output / "noise.tsv", noise)


if __name__ == "__main__":
    main()
