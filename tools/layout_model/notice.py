"""Write the layout model's NOTICE.md from the provenance.json of prepare_vocab.py.

Offline tool. Usage: notice.py PROVENANCE.json [MODEL_FILE_NAME] > NOTICE.md

Every source the run used is named with its version or SHA-256. wordfreq distributes SUBTLEX word
lists on condition that their authors are credited and that it stays clear SUBTLEX is freely
available data, so the lists in the wordfreq languages used are credited with their citations
(from wordfreq's README).
"""
import json
import sys
import textwrap

NAMES = {"en": "English", "ru": "Russian", "et": "Estonian", "de": "German", "fr": "French", "es": "Spanish",
         "nl": "Dutch", "zh": "Chinese"}
SUBTLEX = {
    "en": (
        ("SUBTLEX-US", "Brysbaert, M. & New, B. (2009). Moving beyond Kucera and Francis: A Critical Evaluation "
         "of Current Word Frequency Norms and the Introduction of a New and Improved Word Frequency Measure for "
         "American English. Behavior Research Methods, 41 (4), 977-990."),
        ("SUBTLEX-UK", "van Heuven, W. J., Mandera, P., Keuleers, E., & Brysbaert, M. (2014). SUBTLEX-UK: A new "
         "and improved word frequency database for British English. The Quarterly Journal of Experimental "
         "Psychology, 67(6), 1176-1190."),
    ),
    "de": (
        ("SUBTLEX-DE", "Brysbaert, M., Buchmeier, M., Conrad, M., Jacobs, A.M., Bölte, J., & Böhl, A. (2011). "
         "The word frequency effect: A review of recent developments and implications for the choice of "
         "frequency estimates in German. Experimental Psychology, 58, 412-424."),
    ),
    "nl": (
        ("SUBTLEX-NL", "Keuleers, E., Brysbaert, M. & New, B. (2010). SUBTLEX-NL: A new frequency measure for "
         "Dutch words based on film subtitles. Behavior Research Methods, 42(3), 643-650."),
    ),
    "zh": (
        ("SUBTLEX-CH", "Cai, Q., & Brysbaert, M. (2010). SUBTLEX-CH: Chinese word and character frequencies based "
         "on film subtitles. PLoS One, 5(6), e10729."),
    ),
}


def name(code):
    if code not in NAMES:
        raise SystemExit(f"notice.py has no name for language {code}; add it to NAMES")
    return NAMES[code]


def listed(items):
    items = list(items)
    return items[0] if len(items) == 1 else ", ".join(items[:-1]) + " and " + items[-1]


def paragraph(text, first="", rest=""):
    return textwrap.fill(text, width=78, initial_indent=first, subsequent_indent=rest,
                         break_long_words=False, break_on_hyphens=False)


def bullet(text):
    return paragraph(text, "- ", "  ")


def render(receipt, model="en-ru-et.aklm"):
    version = receipt["wordfreq"]
    languages = [(entry["id"].split("-")[0].lower(), entry) for entry in receipt["languages"]]
    from_wordfreq = [code for code, entry in languages if entry["frequency"].startswith("wordfreq:")]
    parts = [
        "# Layout model data notice",
        paragraph(f"`{model}` is a trained model derived from word-frequency data. It is distributed under the "
                  "Creative Commons Attribution-ShareAlike 4.0 International license "
                  "(<https://creativecommons.org/licenses/by-sa/4.0/>), the license of that data. Code that "
                  "reads it keeps the project's license."),
        "Sources used for training:",
    ]
    sources = []
    if from_wordfreq:
        sources.append(bullet(
            f"{listed(name(code) for code in from_wordfreq)} word frequencies: wordfreq {version} by Robyn Speer "
            "(<https://github.com/rspeer/wordfreq>), data licensed CC BY-SA 4.0. wordfreq includes data from "
            "Google Books Ngrams, the Leeds Internet Corpus, Wikipedia, ParaCrawl and other sources listed in "
            "its NOTICE."))
    for code, entry in languages:
        if entry["frequency"].startswith("file:"):
            if not entry.get("source") or not entry.get("sha256"):
                raise SystemExit(f"{entry['id']}: the receipt names no source and SHA-256 for {entry['frequency']}")
            sources.append(bullet(f"{name(code)} word frequencies: {entry['source']}, SHA-256 `{entry['sha256']}`."))
    if receipt["noise"]:
        sources.append(bullet(f"{listed(name(code) for code in receipt['noise'])} words from wordfreq {version}, "
                              "used only as text that must stay unchanged."))
    sources.append(bullet(f"Dictionary membership features use the {listed(name(code) for code, _ in languages)} "
                          "word lists already documented in `data/language-packs/`."))
    parts.append("\n".join(sources))
    credited = [(code, lists) for code, lists in SUBTLEX.items() if code in from_wordfreq + receipt["noise"]]
    if credited:
        names = listed(f"{', '.join(title for title, _ in lists)} for {name(code)}" for code, lists in credited)
        parts.append(paragraph(
            f"The wordfreq data used here includes SUBTLEX word lists ({names}) created by Marc Brysbaert et al. "
            "SUBTLEX is freely available data (<http://crr.ugent.be/programs-data/subtitle-frequencies>), which "
            "wordfreq distributes with his permission on condition that its authors are credited:"))
        parts.append("\n".join(bullet(citation) for _, lists in credited for _, citation in lists))
    parts.append(paragraph("The model contains no text typed by any user. It is built with "
                           "`tools/layout_model/pipeline.sh`; see `docs/layout-model.md` for the steps and for what "
                           "a rebuild reproduces."))
    return "\n\n".join(parts) + "\n"


def main():
    if len(sys.argv) not in (2, 3):
        raise SystemExit(__doc__)
    receipt = json.loads(open(sys.argv[1], encoding="utf-8").read())
    sys.stdout.write(render(receipt, *sys.argv[2:]))


if __name__ == "__main__":
    main()
