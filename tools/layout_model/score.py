"""Score layout decisions against generated cases.

Usage: score.py CASES.jsonl PREDICTIONS.jsonl [PREDICTIONS.jsonl ...]
Prediction lines: {"id": N, "target": layout | null}; null keeps the text.

A decision is correct when the resulting text equals the intended text.
"false" = text that was already correct got changed (the costly error),
"fixed" = wrong-layout text became correct, "missed" = left unchanged,
"wrong" = converted into another wrong text.
"""
import json
import sys
from collections import defaultdict


def bucket(length):
    if length <= 3:
        return f"len{length}"
    return "len4-6" if length <= 6 else "len7+"


def main():
    cases = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
    for path in sys.argv[2:]:
        predictions = {}
        for line in open(path, encoding="utf-8"):
            record = json.loads(line)
            predictions[record["id"]] = record["target"]
        stats = defaultdict(lambda: defaultdict(int))
        for case in cases:
            target = predictions[case["id"]]
            result = case["candidates"][target or case["typed"]]
            needs = case["label"] != case["typed"]
            if not needs:
                outcome = "ok" if result == case["intended"] else "false"
            elif result == case["intended"]:
                outcome = "fixed"
            else:
                outcome = "missed" if target is None or target == case["typed"] else "wrong"
            groups = ["all", f"kind:{case['kind']}", bucket(len(case["intended"]))]
            if case["unseen"]:
                groups.append("unseen")
            if needs:
                groups.append(f"pair:{case['typed']}>{case['label']}")
            for group in groups:
                stats[group][outcome] += 1
        print(f"== {path}")
        print(f"{'group':<22}{'keep n':>8}{'false%':>8}{'conv n':>8}{'fixed%':>8}{'missed%':>8}{'wrong%':>8}")
        for group in sorted(stats, key=lambda g: (g != "all", g)):
            s = stats[group]
            keep = s["ok"] + s["false"]
            conv = s["fixed"] + s["missed"] + s["wrong"]
            pct = lambda a, b: f"{100 * a / b:7.2f}" if b else "      -"
            print(
                f"{group:<22}{keep:>8}{pct(s['false'], keep):>8}{conv:>8}"
                f"{pct(s['fixed'], conv):>8}{pct(s['missed'], conv):>8}{pct(s['wrong'], conv):>8}"
            )


if __name__ == "__main__":
    main()
