import json
import random
import struct
import tempfile
import unittest
from pathlib import Path

import numpy as np

from bopogram.charlm_native import ALIGN, MAGIC, NativeCharLM, quantize


def write_model(path: Path, d=8, heads=2, ff=16, chars=20, layers=2, max_len=16, seed=0):
    """A small random model in the engine's format (same layout as export_charlm_native.py)."""
    rng = np.random.default_rng(seed)
    f = lambda *shape: rng.normal(0, 0.5, shape).astype(np.float32)

    def q(w):
        s = (np.abs(w).max(axis=1) / 127).astype(np.float32)
        return np.clip(np.round(w / s[:, None]), -127, 127).astype(np.int8), s

    tensors = [("emb.q", q(f(chars, d))[0]), ("emb.scale", q(f(chars, d))[1]), ("pos", f(max_len, d))]
    for i in range(layers):
        tensors += [(f"{i}.norm1.w", f(d) + 1), (f"{i}.norm1.b", f(d)), (f"{i}.norm2.w", f(d) + 1), (f"{i}.norm2.b", f(d))]
        for name, shape in (("qkv", (3 * d, d)), ("proj", (d, d)), ("ff1", (ff, d)), ("ff2", (d, ff))):
            wq, ws = q(f(*shape))
            tensors += [(f"{i}.{name}.q", wq), (f"{i}.{name}.scale", ws), (f"{i}.{name}.b", f(shape[0]))]
    tensors += [("norm.w", f(d) + 1), ("norm.b", f(d)), ("out_bias", f(chars))]
    index, off = [], 0
    for name, arr in tensors:
        off = (off + ALIGN - 1) // ALIGN * ALIGN
        index.append({"name": name, "dtype": "i8" if arr.dtype == np.int8 else "f32", "shape": list(arr.shape), "offset": off})
        off += arr.nbytes
    head = json.dumps({"format": 1, "d_model": d, "n_layers": layers, "n_heads": heads, "d_ff": ff,
                       "n_chars": chars, "max_len": max_len, "ln_eps": 1e-5, "tensors": index}).encode()
    start = (12 + len(head) + ALIGN - 1) // ALIGN * ALIGN
    with open(path, "wb") as fh:
        fh.write(MAGIC + struct.pack("<I", len(head)) + head)
        for (_, arr), e in zip(tensors, index):
            fh.seek(start + e["offset"])
            fh.write(arr.tobytes())


class NativeCharLMTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.TemporaryDirectory()
        cls.path = Path(cls.dir.name) / "charlm.bin"
        write_model(cls.path)

    @classmethod
    def tearDownClass(cls):
        cls.dir.cleanup()

    def test_prefix_cache_gives_the_same_numbers(self):
        cached = NativeCharLM(self.path)
        rng = random.Random(0)
        base = [1] + [rng.randrange(2, 20) for _ in range(12)]
        for _ in range(40):
            seq = base[: rng.randrange(2, 12)] + [rng.randrange(2, 20) for _ in range(rng.randrange(1, 4))]
            first = rng.randrange(0, len(seq) - 1)
            self.assertEqual(cached.logp(seq, first), NativeCharLM(self.path).logp(seq, first))

    def test_scores_are_log_probabilities(self):
        m = NativeCharLM(self.path)
        total = sum(np.exp(m.logp([1, c], 0)) for c in range(20))
        self.assertAlmostEqual(total, 1.0, places=5)

    def test_activation_quantization(self):
        q, s = quantize(np.array([[0.5, -1.27, 0.0049], [0, 0, 0]], dtype=np.float32))
        self.assertEqual(q[0].tolist(), [50.0, -127.0, 0.0])
        self.assertEqual(q[1].tolist(), [0.0, 0.0, 0.0])
        self.assertEqual(s[1], 0)


if __name__ == "__main__":
    unittest.main()
