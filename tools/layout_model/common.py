"""Shared configuration helpers for the layout-model tools."""
import gzip
import json
from pathlib import Path

CONFIG = Path(__file__).with_name("languages.json")


def load_config(path=CONFIG):
    config = json.loads(Path(path).read_text(encoding="utf-8"))
    ids = [language["id"] for language in config["languages"]]
    if len(set(ids)) != len(ids) or not 2 <= len(ids) <= 16:
        raise SystemExit("languages.json needs 2-16 distinct languages")
    if config["noise_layout"] not in ids:
        raise SystemExit("noise_layout must be one of the languages")
    return config


def layout_ids(config):
    return [language["id"] for language in config["languages"]]


def load_layouts(path):
    """Exported Windows layouts keyed by LANGID (hex string)."""
    data = json.loads(Path(path).read_text(encoding="utf-8-sig"))
    return {layout["language_id"]: layout for layout in data["layouts"]}


def key_maps(config, layouts_path):
    """Per layout id: character -> (scan code, shift) and the inverse."""
    layouts = load_layouts(layouts_path)
    to_key, to_char = {}, {}
    for language in config["languages"]:
        exported = layouts.get(language["language_id"])
        if exported is None:
            raise SystemExit(f"layout {language['language_id']} ({language['id']}) was not exported")
        forward, backward = {}, {}
        for code, key in exported["keys"].items():
            for shift, value in ((False, key["normal"]), (True, key["shift"])):
                if value is not None:
                    backward.setdefault(value, (code, shift))
                    forward[(code, shift)] = value
        to_key[language["id"]], to_char[language["id"]] = backward, forward
    return to_key, to_char


def load_dictionaries(config, directory):
    """Per layout id: (main words, short words), lowercase."""
    directory = Path(directory)
    result = {}
    for language in config["languages"]:
        sets = []
        for relative in language["dictionary"]:
            path = directory / relative
            opener = gzip.open if path.suffix == ".gz" else open
            with opener(path, "rt", encoding="utf-8") as stream:
                sets.append({line.strip().lower() for line in stream if line.strip()})
        result[language["id"]] = tuple(sets)
    return result
