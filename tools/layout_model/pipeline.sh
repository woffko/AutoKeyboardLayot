#!/usr/bin/env bash
# Build, evaluate and export the layout model for the languages in
# languages.json. Offline; writes only under WORK (default target/layout-model).
#
# Prerequisites (see docs/layout-model.md):
#   WORK/layouts.json   exported on Windows with export_layouts.ps1
#   WORK/sources/       frequency lists referenced as file:NAME
#   WORK/dicts/         dictionaries referenced in languages.json
#   WORK/store-copy/    a copy of an installed package store (baseline only)
#   Python with torch, numpy and wordfreq (PYTHON, default WORK/.venv/bin/python)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="${WORK:-$ROOT/target/layout-model}"
PYTHON="${PYTHON:-$WORK/.venv/bin/python}"
TOOLS="$ROOT/tools/layout_model"
THRESHOLD="${THRESHOLD:-0.9}"
OUTPUT="${OUTPUT:-$WORK/model.aklm}"
cd "$TOOLS"

"$PYTHON" prepare_vocab.py "$WORK/layouts.json" "$WORK/sources" "$WORK/vocab"
for shard in 0 1 2 3 4 5 6 7; do
    "$PYTHON" generate_cases.py "$WORK/layouts.json" "$WORK/vocab" "$WORK/train.$shard.jsonl" 500000 train $((100 + shard)) &
done
"$PYTHON" generate_cases.py "$WORK/layouts.json" "$WORK/vocab" "$WORK/test.jsonl" 200000 test 2 &
"$PYTHON" generate_cases.py "$WORK/layouts.json" "$WORK/vocab" "$WORK/uniform.jsonl" 100000 uniform 3 &
wait
cat "$WORK"/train.[0-7].jsonl | awk '{ sub(/"id": [0-9]+/, "\"id\": " NR - 1); print }' > "$WORK/train.jsonl"
rm -f "$WORK"/train.[0-7].jsonl

for set in train test uniform; do
    "$PYTHON" train.py featurize "$WORK/dicts" "$WORK/$set.jsonl" "$WORK/$set.npz"
done
"$PYTHON" train.py fit "$WORK/train.npz" "$WORK/model.pt"
"$PYTHON" export.py "$WORK/model.pt" "$OUTPUT"
"$PYTHON" export.py check "$OUTPUT" "$WORK/dicts" "$WORK/test.jsonl" "$WORK/test.model.jsonl" "$THRESHOLD" "$WORK/fixture.json"

if [ -d "$WORK/store-copy" ]; then
    for set in test uniform; do
        (cd "$ROOT" && cargo run -q --release --example evaluate_layout_detection -- \
            "$WORK/store-copy" "$WORK/$set.jsonl" "$WORK/$set.baseline.jsonl")
        (cd "$ROOT" && cargo run -q --release --example evaluate_layout_detection -- \
            "$WORK/store-copy" "$WORK/$set.jsonl" "$WORK/$set.agent.jsonl" --model "$OUTPUT")
        "$PYTHON" score.py "$WORK/$set.jsonl" "$WORK/$set.baseline.jsonl" "$WORK/$set.agent.jsonl"
    done
fi
