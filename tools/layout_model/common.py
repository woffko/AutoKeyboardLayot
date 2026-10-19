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


def exported_klid(layout):
    """The KLID of an exported layout: as export_layouts.ps1 writes it, or read from an HKL whose
    high word is a KLID's own number (00000409 for 0x04090409); None when neither tells."""
    if layout.get("klid"):
        return layout["klid"].upper()
    if layout.get("hkl"):
        device = (int(layout["hkl"], 16) >> 16) & 0xFFFF
        if device & 0xF000 != 0xF000:
            return f"{device:08X}"
    return None


def select_layouts(config, path):
    """Per language of languages.json: the exported layout it uses.

    A language names its exact layout by KLID ("layout"); an export without KLIDs falls back to
    the language's LANGID. Exactly one exported layout must fit: US and US-International share
    LANGID 0409, and taking either silently trains the model on the wrong keys.
    """
    exported = json.loads(Path(path).read_text(encoding="utf-8-sig"))["layouts"]
    klids = [exported_klid(layout) for layout in exported]
    for klid in set(klids) - {None}:
        if klids.count(klid) > 1:
            raise SystemExit(f"layout {klid} was exported twice")
    selected = {}
    for language in config["languages"]:
        wanted = (language.get("layout") or "").upper()
        if wanted and any(klids):
            fits = [layout for layout, klid in zip(exported, klids) if klid == wanted]
        else:
            fits = [layout for layout in exported if layout["language_id"] == language["language_id"]]
        if len(fits) != 1:
            raise SystemExit(
                f"{language['id']}: {len(fits)} exported layouts fit {wanted or language['language_id']}; "
                "name its KLID as \"layout\" in languages.json and export the layouts again")
        selected[language["id"]] = fits[0]
    return selected


def key_maps(config, layouts_path):
    """Per layout id: character -> (scan code, shift) and the inverse."""
    layouts = select_layouts(config, layouts_path)
    to_key, to_char = {}, {}
    for language in config["languages"]:
        exported = layouts[language["id"]]
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
