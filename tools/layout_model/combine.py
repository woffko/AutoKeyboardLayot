"""Combine dictionary-detector and model decisions into a hybrid policy.

Usage: combine.py CASES.jsonl BASELINE.jsonl MODEL.jsonl OUTPUT.jsonl MIN_MODEL_LENGTH

Policy: the dictionary detector's conversion always wins (it is the proven,
low-risk path). For words of at least MIN_MODEL_LENGTH characters that the
detector leaves unchanged, the model decision is used. Shorter words keep the
detector's list-based policy.
"""
import json
import sys


def load(path):
    return {r["id"]: r["target"] for r in map(json.loads, open(path, encoding="utf-8"))}


def main():
    cases_path, baseline_path, model_path, output, minimum = sys.argv[1:6]
    baseline, model = load(baseline_path), load(model_path)
    with open(output, "w", encoding="utf-8") as stream:
        for case in map(json.loads, open(cases_path, encoding="utf-8")):
            target = baseline[case["id"]]
            if target is None and len(case["candidates"][case["typed"]]) >= int(minimum):
                target = model[case["id"]]
            stream.write(json.dumps({"id": case["id"], "target": target}) + "\n")


if __name__ == "__main__":
    main()
