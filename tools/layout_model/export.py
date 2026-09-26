"""Export a trained layout model to the agent's binary format and check it.

Offline tool.

Usage:
  export.py MODEL.pt OUTPUT.aklm
  export.py check MODEL.aklm DICT_DIR CASES.jsonl PREDICTIONS.jsonl THRESHOLD FIXTURE.json
    Runs the quantized reference inference (exactly what the agent computes),
    writes gated predictions and a fixture of expected probabilities used by
    the Rust unit test.

Format (little endian), version 2:
  b"AKLM", u16 version, u8 layout count N, per layout (u8 length, ASCII id),
  u32 buckets, u16 dim, u16 hidden, u8 max_n, u8 max_ngrams, u8 context,
  buckets rows of (f32 scale, dim x i8), hidden weight
  [hidden x (dim + 5 + 2N)] f32 row-major, hidden bias [hidden] f32, output
  weight [hidden] f32, output bias f32. Feature order: see train.py.
"""
import json
import struct
import sys

import numpy as np

from common import load_config, load_dictionaries
from train import CONTEXT, MAX_N, MAX_NGRAMS, context_weights, dense_features, ngram_ids

VERSION = 2


def export(model_path, output):
    import torch

    saved = torch.load(model_path)
    state = {k: v.cpu().numpy() for k, v in saved["state"].items()}
    buckets, dim, layouts = saved["buckets"], saved["dim"], saved["layouts"]
    embedding = state["embedding.weight"][:buckets]
    hidden_w, hidden_b = state["hidden.weight"], state["hidden.bias"]
    out_w, out_b = state["output.weight"][0], state["output.bias"][0]
    blob = bytearray(b"AKLM") + struct.pack("<HB", VERSION, len(layouts))
    for layout in layouts:
        blob += struct.pack("<B", len(layout)) + layout.encode("ascii")
    blob += struct.pack("<IHHBBB", buckets, dim, hidden_w.shape[0], MAX_N, MAX_NGRAMS, CONTEXT)
    scales = np.abs(embedding).max(axis=1) / 127.0
    scales[scales == 0] = 1.0
    quantized = np.clip(np.round(embedding / scales[:, None]), -127, 127).astype(np.int8)
    for scale, row in zip(scales.astype(np.float32), quantized):
        blob += struct.pack("<f", scale) + row.tobytes()
    blob += hidden_w.astype("<f4").tobytes() + hidden_b.astype("<f4").tobytes()
    blob += out_w.astype("<f4").tobytes() + struct.pack("<f", out_b)
    open(output, "wb").write(bytes(blob))
    print(f"{output}: {len(blob)} bytes, layouts {layouts}")


class Reference:
    def __init__(self, path):
        data = open(path, "rb").read()
        assert data[:4] == b"AKLM"
        version, count = struct.unpack_from("<HB", data, 4)
        assert version == VERSION
        offset, self.layouts = 7, []
        for _ in range(count):
            length = data[offset]
            self.layouts.append(data[offset + 1 : offset + 1 + length].decode())
            offset += 1 + length
        self.buckets, self.dim, self.hidden, _, _, _ = struct.unpack_from("<IHHBBB", data, offset)
        offset += 11
        rows = np.frombuffer(data, np.uint8, self.buckets * (4 + self.dim), offset).reshape(self.buckets, 4 + self.dim)
        scales = rows[:, :4].copy().view("<f4")[:, 0]
        self.embedding = rows[:, 4:].copy().view(np.int8).astype(np.float32) * scales[:, None]
        offset += self.buckets * (4 + self.dim)
        width = self.dim + 5 + 2 * count
        self.hidden_w = np.frombuffer(data, "<f4", self.hidden * width, offset).reshape(self.hidden, width)
        offset += self.hidden * width * 4
        self.hidden_b = np.frombuffer(data, "<f4", self.hidden, offset)
        offset += self.hidden * 4
        self.out_w = np.frombuffer(data, "<f4", self.hidden, offset)
        offset += self.hidden * 4
        (self.out_b,) = struct.unpack_from("<f", data, offset)
        assert offset + 4 == len(data)

    def probabilities(self, candidates, typed, dictionaries, context):
        weights = context_weights(self.layouts, context)
        typed_text = candidates[typed]
        logits = np.full(len(self.layouts), -np.inf, dtype=np.float32)
        for index, layout in enumerate(self.layouts):
            text = candidates.get(layout)
            if text is None:
                continue
            ids = ngram_ids(index, text, self.buckets)
            embedded = self.embedding[ids].mean(axis=0) if ids else np.zeros(self.dim, np.float32)
            dense = dense_features(self.layouts, index, text, typed, typed_text, dictionaries, weights)
            hidden = np.maximum(self.hidden_w @ np.concatenate([embedded, dense]) + self.hidden_b, 0)
            logits[index] = hidden @ self.out_w + self.out_b
        exp = np.exp(logits - logits.max())
        return exp / exp.sum()


def check(model, dict_dir, cases_path, predictions, threshold, fixture):
    reference = Reference(model)
    dictionaries = load_dictionaries(load_config(), dict_dir)
    threshold = float(threshold)
    fixtures = []
    with open(predictions, "w", encoding="utf-8") as out:
        for case in map(json.loads, open(cases_path, encoding="utf-8")):
            typed = case["typed"]
            probs = reference.probabilities(case["candidates"], typed, dictionaries, case["context"])
            best = int(probs.argmax())
            layout = reference.layouts[best]
            main, short = dictionaries[typed]
            typed_text = case["candidates"][typed]
            known = typed_text.lower() in main or typed_text.lower() in short
            convert = case["candidates"][layout] != typed_text and probs[best] >= threshold and not known
            out.write(json.dumps({"id": case["id"], "target": layout if convert else None}) + "\n")
            if len(fixtures) < 40 and case["id"] % 97 == 0:
                fixtures.append(
                    {
                        "typed": typed,
                        "candidates": case["candidates"],
                        "context": case["context"],
                        "flags": {
                            layout: [text.lower() in dictionaries[layout][0], text.lower() in dictionaries[layout][1]]
                            for layout, text in case["candidates"].items()
                            if text is not None
                        },
                        "probabilities": [float(p) for p in probs],
                    }
                )
    json.dump(fixtures, open(fixture, "w", encoding="utf-8"), ensure_ascii=False, indent=1)
    print(f"{predictions}; fixture {fixture}: {len(fixtures)} cases")


if __name__ == "__main__":
    if sys.argv[1] == "check":
        check(*sys.argv[2:8])
    else:
        export(*sys.argv[1:3])
