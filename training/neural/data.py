"""Training batches for the position-wise model, sampled from the arrays of make_data.py.

Each example imitates one moment of typing: some characters of a sentence are
already committed (left context, given as characters), the following run of
Chinese characters is the composition buffer (given as syllables, to be
predicted), and nothing after the buffer is known.

The arrays are memory-mapped; nothing is loaded whole.
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import torch

HOLDOUT = 2000  # first sentences of the first shard of every source: validation only
MAX_CONTEXT = 40
MAX_SPAN = 32
SOURCE_WEIGHT = {"zhwiki": 0.40, "ly_gazette": 0.33, "judicial": 0.07, "ey_news": 0.06,
                 "twllm": 0.06, "ey": 0.04, "zhwikinews": 0.04}


class Shard:
    def __init__(self, stem: str, first: bool):
        self.chars = np.load(stem + ".chars.npy", mmap_mode="r")
        self.syls = np.load(stem + ".syls.npy", mmap_mode="r")
        self.offs = np.load(stem + ".offs.npy", mmap_mode="r")
        self.lo = HOLDOUT if first else 0  # first training sentence
        self.n = len(self.offs) - 1


class Corpus:
    def __init__(self, data_dir: Path):
        self.vocab = json.loads((data_dir / "vocab.json").read_text(encoding="utf-8"))
        self.sources: dict[str, list[Shard]] = {}
        for src_dir in sorted(p for p in data_dir.iterdir() if p.is_dir()):
            stems = sorted(str(p)[: -len(".offs.npy")] for p in src_dir.glob("part-*.offs.npy"))
            if stems:
                self.sources[src_dir.name] = [Shard(s, i == 0) for i, s in enumerate(stems)]
        names = list(self.sources)
        w = np.array([SOURCE_WEIGHT.get(n, 0.02) for n in names])
        self.names, self.weights = names, w / w.sum()
        self.shard_p = {n: np.array([s.n - s.lo for s in sh], dtype=np.float64) for n, sh in self.sources.items()}
        for n in names:
            self.shard_p[n] /= self.shard_p[n].sum()

    def sentence(self, shard: Shard, k: int) -> tuple[np.ndarray, np.ndarray]:
        a, b = int(shard.offs[k]), int(shard.offs[k + 1])
        return np.asarray(shard.chars[a:b]), np.asarray(shard.syls[a:b])

    def heldout(self):
        for name, shards in self.sources.items():
            for k in range(min(HOLDOUT, shards[0].n)):
                yield name, *self.sentence(shards[0], k)


def make_example(chars: np.ndarray, syls: np.ndarray, rng: np.random.Generator):
    """Pick a composition buffer inside one sentence. Returns (chars, syls, span_start) or None."""
    readable = np.flatnonzero(syls)
    if len(readable) == 0:
        return None
    end = int(readable[rng.integers(len(readable))]) + 1
    # Typing buffers are short most of the time; long ones occur but are rare.
    want = int(min(MAX_SPAN, 1 + rng.geometric(0.08)))
    start = end
    while start > 0 and end - start < want and syls[start - 1]:
        start -= 1
    r = rng.random()
    if r < 0.25:
        ctx = 0                                   # no context (new field, unsupported app)
    elif r < 0.45:
        ctx = int(rng.integers(0, min(start, MAX_CONTEXT) + 1))  # partial context
    else:
        ctx = min(start, MAX_CONTEXT)
    return chars[start - ctx:end], syls[start - ctx:end], ctx


class Batches(torch.utils.data.IterableDataset):
    """Endless stream of padded batches (already tensors), one stream per worker.

    Examples are drawn in pools, sorted by length and cut into batches of at most
    `max_tokens` padded positions, so little computation is spent on padding.
    """

    POOL = 20000

    def __init__(self, data_dir: Path, max_tokens: int, tone_drop: float, seed: int):
        self.data_dir, self.max_tokens, self.tone_drop, self.seed = data_dir, max_tokens, tone_drop, seed

    def __iter__(self):
        info = torch.utils.data.get_worker_info()
        rng = np.random.default_rng([self.seed, info.id if info else 0])
        corpus = Corpus(self.data_dir)
        while True:
            items = []
            while len(items) < self.POOL:
                name = corpus.names[rng.choice(len(corpus.names), p=corpus.weights)]
                shards = corpus.sources[name]
                shard = shards[rng.choice(len(shards), p=corpus.shard_p[name])]
                k = int(rng.integers(shard.lo, shard.n))
                ex = make_example(*corpus.sentence(shard, k), rng)
                if ex is not None:
                    items.append(ex)
            items.sort(key=lambda ex: len(ex[0]))
            batches, cur = [], []
            for ex in items:
                if cur and (len(cur) + 1) * len(ex[0]) > self.max_tokens:
                    batches.append(cur)
                    cur = []
                cur.append(ex)
            batches.append(cur)
            for i in rng.permutation(len(batches)):
                yield collate(batches[i], rng, self.tone_drop)


def collate(items, rng: np.random.Generator | None, tone_drop: float):
    """-> chars, syls, lengths, drop_tone (all LongTensor/BoolTensor); syls is 0 outside the buffer.
    `chars` keeps the true characters everywhere; the training step hides those in the buffer."""
    n = max(len(c) for c, _, _ in items)
    chars = np.zeros((len(items), n), dtype=np.int64)
    syls = np.zeros((len(items), n), dtype=np.int64)
    lengths = np.zeros(len(items), dtype=np.int64)
    for i, (c, s, ctx) in enumerate(items):
        chars[i, : len(c)] = c
        syls[i, ctx: len(c)] = s[ctx:]
        lengths[i] = len(c)
    if rng is not None and tone_drop > 0:
        # Tones are withheld for a whole buffer (a toneless typist) or for single syllables.
        whole = rng.random(len(items)) < tone_drop / 2
        drop = (rng.random(chars.shape) < tone_drop / 2) | whole[:, None]
    else:
        drop = np.zeros(chars.shape, dtype=bool)
    return (torch.from_numpy(chars), torch.from_numpy(syls), torch.from_numpy(lengths), torch.from_numpy(drop))
