"""Training of the layout model is repeatable and says how it was done.

Needs numpy and torch (the environment of docs/layout-model.md); without them the tests are
skipped. Run them with that environment's Python:
  target/layout-model/.venv/bin/python -m unittest tools/test_layout_model_training.py
They train only on the CPU, on a tiny synthetic set.
"""
import hashlib
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
HAVE_TRAINING = all(importlib.util.find_spec(name) for name in ('numpy', 'torch'))


@unittest.skipUnless(HAVE_TRAINING, 'needs numpy and torch')
class Training(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        sys.path.insert(0, str(TOOLS / 'layout_model'))
        import numpy
        import train
        cls.np, cls.train = numpy, train
        cls.scratch = tempfile.TemporaryDirectory()
        cls.cases = Path(cls.scratch.name) / 'train.npz'
        rng = numpy.random.default_rng(0)
        count, layouts, buckets = 256, 3, 64
        target = numpy.zeros((count, layouts), dtype=bool)
        target[numpy.arange(count), rng.integers(0, layouts, count)] = True
        numpy.savez(cls.cases,
                    ngrams=rng.integers(0, buckets + 1, (count, layouts, train.MAX_NGRAMS)).astype(numpy.int32),
                    dense=rng.random((count, layouts, 5 + 2 * layouts)).astype(numpy.float32),
                    present=numpy.ones((count, layouts), dtype=bool), target=target, buckets=buckets)

    @classmethod
    def tearDownClass(cls):
        cls.scratch.cleanup()

    def fit(self, name, seed):
        import torch

        output = Path(self.scratch.name) / name
        self.train.fit(self.cases, output, dim=4, epochs=2, seed=seed, device='cpu')
        return torch.load(output)['state'], output

    def test_the_same_seed_trains_the_same_model_and_another_seed_another(self):
        import torch

        first, _ = self.fit('first.pt', 7)
        second, _ = self.fit('second.pt', 7)
        other, _ = self.fit('other.pt', 8)
        self.assertEqual(first.keys(), second.keys())
        for key in first:
            self.assertTrue(torch.equal(first[key], second[key]), key)
        self.assertFalse(all(torch.equal(first[key], other[key]) for key in first))

    def test_the_receipt_names_the_seed_versions_and_hashes_and_follows_the_export(self):
        import export
        import torch

        _, output = self.fit('receipt.pt', 11)
        receipt = json.loads(Path(f'{output}.json').read_text(encoding='utf-8'))
        digest = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
        self.assertEqual((receipt['seed'], receipt['epochs'], receipt['dim'], receipt['device']), (11, 2, 4, 'cpu'))
        self.assertTrue(receipt['deterministic'])
        self.assertEqual(receipt['torch'], torch.__version__)
        self.assertEqual(receipt['numpy'], self.np.__version__)
        self.assertEqual(receipt['train_sha256'], digest(self.cases))
        self.assertEqual(receipt['languages_json_sha256'], digest(TOOLS / 'layout_model' / 'languages.json'))
        self.assertEqual(receipt['model_sha256'], digest(output))
        exported = Path(self.scratch.name) / 'receipt.aklm'
        export.export(output, exported)
        carried = json.loads(Path(f'{exported}.json').read_text(encoding='utf-8'))
        self.assertEqual(carried['aklm_sha256'], digest(exported))
        self.assertEqual({key: value for key, value in carried.items() if key != 'aklm_sha256'}, receipt)


if __name__ == '__main__':
    unittest.main()
