"""Train and apply the character-level layout model.

Offline tool. Nothing here is shipped or read by the agent.

Usage:
  train.py featurize DICT_DIR CASES.jsonl OUTPUT.npz [BUCKETS]
  train.py fit TRAIN.npz OUTPUT.pt [--dim N] [--epochs N]
  train.py predict MODEL.pt CASES.npz CASES.jsonl OUTPUT_PREFIX THRESHOLD [...]

Languages come from languages.json (N of them, in model order). For every
layout reading of the typed keys the model embeds hashed character n-grams
(n = 1..4, one hash space per layout), appends dense features and scores it
with a small MLP; a softmax over the readings gives the intended layout.

Dense features per reading (5 + 2N): in main dictionary, in short list, is
the typed layout, equals the typed text, context weight of this layout,
layout one-hot (N), context weights of all layouts (N). Context weight of a
layout = sum of 0.5^age over the previous four words that ended in it.

predict mirrors the agent: a word known in the typed layout is never
converted, and a conversion needs probability >= THRESHOLD.
Hashing uses CRC-32 of (layout index + 1 as one byte, UTF-8 n-gram), which the
agent reproduces without dependencies.
"""
import json
import sys
import zlib
from multiprocessing import Pool

import numpy as np

from common import layout_ids, load_config, load_dictionaries

MAX_NGRAMS = 80
MAX_N = 4
CONTEXT = 4
DEFAULT_BUCKETS = 1 << 16
HIDDEN = 64

_state = {}


def ngram_ids(layout_index, text, buckets):
    padded = f"<{text.lower()}>"
    prefix = bytes([layout_index + 1])
    ids = []
    for n in range(1, MAX_N + 1):
        for start in range(len(padded) - n + 1):
            gram = padded[start : start + n]
            if gram in ("<", ">"):
                continue
            ids.append(zlib.crc32(prefix + gram.encode("utf-8")) % buckets)
    return ids[:MAX_NGRAMS]


def context_weights(layouts, context):
    weights = np.zeros(len(layouts), dtype=np.float32)
    for age, previous in enumerate(reversed(context[-CONTEXT:])):
        if previous in layouts:
            weights[layouts.index(previous)] += 0.5**age
    return weights


def dense_features(layouts, index, text, typed, typed_text, dictionaries, context):
    main, short = dictionaries[layouts[index]]
    lower = text.lower()
    return np.concatenate(
        [
            [float(lower in main), float(lower in short), float(layouts[index] == typed), float(text == typed_text), context[index]],
            np.eye(len(layouts), dtype=np.float32)[index],
            context,
        ]
    ).astype(np.float32)


def featurize_line(args):
    line, buckets = args
    layouts, dictionaries = _state["layouts"], _state["dictionaries"]
    count = len(layouts)
    case = json.loads(line)
    ngrams = np.full((count, MAX_NGRAMS), buckets, dtype=np.int32)
    dense = np.zeros((count, 5 + 2 * count), dtype=np.float32)
    present = np.zeros(count, dtype=bool)
    target = np.zeros(count, dtype=bool)
    context = context_weights(layouts, case["context"])
    typed = case["typed"]
    typed_text = case["candidates"][typed]
    intended_text = case["candidates"][case["label"]]
    for index, layout in enumerate(layouts):
        text = case["candidates"].get(layout)
        if text is None:
            continue
        present[index] = True
        target[index] = text == intended_text
        ids = ngram_ids(index, text, buckets)
        ngrams[index, : len(ids)] = ids
        dense[index] = dense_features(layouts, index, text, typed, typed_text, dictionaries, context)
    return ngrams, dense, present, target


def init_worker(directory):
    config = load_config()
    _state["layouts"] = layout_ids(config)
    _state["dictionaries"] = load_dictionaries(config, directory)


def featurize(directory, cases, output, buckets=DEFAULT_BUCKETS):
    with open(cases, encoding="utf-8") as stream:
        lines = stream.readlines()
    with Pool(48, initializer=init_worker, initargs=(directory,)) as pool:
        rows = pool.map(featurize_line, ((line, buckets) for line in lines), chunksize=4096)
    arrays = [np.stack(column) for column in zip(*rows)]
    np.savez(output, ngrams=arrays[0], dense=arrays[1], present=arrays[2], target=arrays[3], buckets=buckets)
    print(f"{output}: {len(rows)} cases")


def build_model(torch, buckets, dim, layouts):
    nn = torch.nn

    class LayoutModel(nn.Module):
        def __init__(self):
            super().__init__()
            self.embedding = nn.EmbeddingBag(buckets + 1, dim, mode="mean", padding_idx=buckets)
            self.hidden = nn.Linear(dim + 5 + 2 * layouts, HIDDEN)
            self.output = nn.Linear(HIDDEN, 1)

        def forward(self, ngrams, dense, present):
            batch, count = ngrams.shape[:2]
            embedded = self.embedding(ngrams.reshape(batch * count, -1)).reshape(batch, count, -1)
            features = torch.cat([embedded, dense], dim=-1)
            logits = self.output(torch.relu(self.hidden(features))).squeeze(-1)
            return logits.masked_fill(~present, -1e9)

    return LayoutModel()


def fit(train_path, output, dim=16, epochs=4):
    import torch

    data = np.load(train_path)
    buckets = int(data["buckets"])
    layouts = layout_ids(load_config())
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    tensors = [torch.from_numpy(data[name]) for name in ("ngrams", "dense", "present", "target")]
    if tensors[0].shape[1] != len(layouts):
        raise SystemExit("features were built for another languages.json")
    model = build_model(torch, buckets, dim, len(layouts)).to(device)
    optimizer = torch.optim.Adam(model.parameters(), lr=3e-3)
    total, batch = tensors[0].shape[0], 4096
    for epoch in range(epochs):
        order = torch.randperm(total)
        running, steps = 0.0, 0
        for start in range(0, total, batch):
            index = order[start : start + batch]
            ngrams, dense, present, target = (t[index].to(device, non_blocking=True) for t in tensors)
            log_probs = torch.log_softmax(model(ngrams.long(), dense, present), dim=-1)
            loss = -torch.logsumexp(log_probs.masked_fill(~target, -1e9), dim=-1).mean()
            optimizer.zero_grad()
            loss.backward()
            optimizer.step()
            running, steps = running + loss.item(), steps + 1
        for group in optimizer.param_groups:
            group["lr"] *= 0.5
        print(f"epoch {epoch + 1}: loss {running / steps:.5f}")
    torch.save({"state": model.state_dict(), "buckets": buckets, "dim": dim, "layouts": layouts}, output)


def predict(model_path, cases_npz, cases_jsonl, prefix, thresholds):
    import torch

    saved = torch.load(model_path)
    layouts = saved["layouts"]
    model = build_model(torch, saved["buckets"], saved["dim"], len(layouts))
    model.load_state_dict(saved["state"])
    model.eval()
    data = np.load(cases_npz)
    tensors = [torch.from_numpy(data[name]) for name in ("ngrams", "dense", "present")]
    probabilities = []
    with torch.no_grad():
        for start in range(0, tensors[0].shape[0], 16384):
            ngrams, dense, present = (t[start : start + 16384] for t in tensors)
            probabilities.append(torch.softmax(model(ngrams.long(), dense, present), -1))
    probabilities = torch.cat(probabilities).numpy()
    dense = data["dense"]
    cases = [json.loads(line) for line in open(cases_jsonl, encoding="utf-8")]
    for threshold in thresholds:
        path = f"{prefix}.t{threshold}.jsonl"
        with open(path, "w", encoding="utf-8") as stream:
            for number, (case, row) in enumerate(zip(cases, probabilities)):
                best = int(row.argmax())
                layout = layouts[best]
                typed = layouts.index(case["typed"])
                known = dense[number, typed, 0] > 0 or dense[number, typed, 1] > 0
                convert = (
                    case["candidates"][layout] != case["candidates"][case["typed"]]
                    and row[best] >= threshold
                    and not known
                )
                stream.write(json.dumps({"id": case["id"], "target": layout if convert else None}) + "\n")
        print(path)


def main():
    command, args = sys.argv[1], sys.argv[2:]
    if command == "featurize":
        featurize(*args[:3], buckets=int(args[3]) if len(args) > 3 else DEFAULT_BUCKETS)
    elif command == "fit":
        options = dict(zip(args[2::2], args[3::2]))
        fit(args[0], args[1], dim=int(options.get("--dim", 16)), epochs=int(options.get("--epochs", 4)))
    elif command == "predict":
        predict(*args[:4], [float(t) for t in args[4:]])
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
