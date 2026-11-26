"""Training data for teaching the character model to choose among the n-gram
decoder's results (prototype/docs/charlm-rerank-training.md).

Usage (from training/):
    uv run python neural/make_rerank_data.py CONFIG DATA_DIR OUT_DIR [--workers 4]
        [--sources ey_news=80000,zhwikinews=50000,...] [--exclude TSV ...]

CONFIG is the runtime config.json (its n-gram models and dictionary; the
character model in it is not loaded). Sentences come from DATA_DIR
(neural/make_data.py output: characters with their readings) of sources the
runtime n-gram models were not trained on. For each sampled sentence the
decoder runs as the engine does before re-ranking (no merging of equal model
states, beam 16, formal style), and the line is written if the right sentence
is among its 5 best and there are at least 2:

    {"src", "context", "gold", "hyps": [{"text", "ngram"}, ...], "label": index of the right one}

Half the sentences get the end of the previous line (up to 40 characters) as
context. A sentence equal to, or containing, a sentence of 8 or more characters
from an --exclude file (column "text") is skipped: the development and test sets.

Checkpoints: the sample is fixed by the seed and split into shards of SHARD
sentences; each shard is written as OUT_DIR/shard-NNNN.jsonl when done, and
existing shards are skipped on a re-run.
"""

from __future__ import annotations

import argparse
import csv
import json
import random
import sys
from multiprocessing import Pool
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "prototype"))

SHARD = 5000
TOP = 5
DEFAULT_SOURCES = "ey_news=80000,zhwikinews=50000,twllm=40000,ey=30000,zhwiki=60000,judicial=40000"
MIN_LEN, MAX_LEN, CONTEXT = 4, 40, 40
HOLDOUT = 2000  # = neural/data.py HOLDOUT: lines the character model never trained on

_engine = None
_cfg_path = None


def sample(data_dir: Path, sources: dict[str, int], exclude: list[str], seed: int, heldout: bool = False):
    """[(source, context, gold, items)] drawn at random from each source. With `heldout`,
    only from the sentences the character model never trained on (the first HOLDOUT
    sentences of each source's first part, neural/data.py); without, never from them."""
    vocab = json.loads((data_dir / "vocab.json").read_text(encoding="utf-8"))
    chars, syls = vocab["chars"], vocab["syllables"]
    special = {i for i, c in enumerate(chars) if c.startswith("<")}
    ex_set = set(exclude)
    by_head: dict[str, list[str]] = {}  # first 8 characters -> excluded sentences
    for t in exclude:
        if len(t) >= 8:
            by_head.setdefault(t[:8], []).append(t)

    def excluded(text: str) -> bool:
        if text in ex_set:
            return True
        return any(t in text for k in range(len(text) - 7) for t in by_head.get(text[k:k + 8], ()))

    rng = random.Random(seed)
    out = []
    for src, want in sources.items():
        parts = sorted((data_dir / src).glob("*.offs.npy"))
        arrays = []
        for p in parts:
            stem = str(p)[: -len(".offs.npy")]
            arrays.append((np.load(stem + ".chars.npy", mmap_mode="r"), np.load(stem + ".syls.npy", mmap_mode="r"),
                           np.load(p, mmap_mode="r")))
        sizes = [len(o) - 1 for _, _, o in arrays]
        total = sum(sizes)
        seen: set[tuple[int, int]] = set()
        got = tries = 0
        limit = min(HOLDOUT, sizes[0]) if heldout else total
        while got < want and tries < 50 * want and len(seen) < limit:
            tries += 1
            # a random sentence of the source: part by size, then a line in it
            if heldout:
                k, r = 0, rng.randrange(min(HOLDOUT, sizes[0]))
            else:
                r = rng.randrange(total)
                k = 0
                while r >= sizes[k]:
                    r -= sizes[k]
                    k += 1
            i = r
            if not heldout and k == 0 and i < HOLDOUT:
                continue
            if (k, i) in seen:
                continue
            seen.add((k, i))
            ch, sy, offs = arrays[k]
            a, b = int(offs[i]), int(offs[i + 1])
            if not MIN_LEN <= b - a <= MAX_LEN:
                continue
            cs, ss = ch[a:b], sy[a:b]
            if any(int(c) in special for c in cs):
                continue
            gold = "".join(chars[int(c)] for c in cs)
            if excluded(gold):
                continue
            items = [syls[int(s)] if s else "=" + chars[int(c)] for c, s in zip(cs, ss)]
            if not any(s for s in ss):
                continue
            context = ""
            if rng.random() < 0.5 and i > 0:
                pa, pb = int(offs[i - 1]), int(offs[i])
                prev = "".join(chars[int(c)] if int(c) not in special else "" for c in ch[pa:pb])
                context = prev[-CONTEXT:]
            out.append((src, context, gold, items))
            got += 1
        print(f"{src}: {got} sentences", flush=True)
    rng.shuffle(out)
    return out


def _init(cfg_path: str) -> None:
    global _engine
    from bopogram.engine import Engine

    cfg = json.loads(Path(cfg_path).read_text(encoding="utf-8"))
    cfg.pop("charlm", None)
    cfg.pop("neural", None)
    tmp = Path(cfg_path).with_name(".rerank-data-config.json")
    if not tmp.exists():
        tmp.write_text(json.dumps(cfg, ensure_ascii=False), encoding="utf-8")
    _engine = Engine(tmp, use_user_model=False)
    _engine.typing_log = None


def _shard(job) -> tuple[str, int, int, int]:
    from bopogram.context import context_state
    from bopogram.evaluate import accept_variants

    path, rows = job
    kept = top1 = 0
    tmp = Path(str(path) + ".tmp")
    with open(tmp, "w", encoding="utf-8") as out:
        for src, context, gold, items in rows:
            dec = _engine.new_decoder()
            dec.merge = False
            dec.style = "formal"
            dec.reset(context_state(context, _engine.lexicon, _engine.lm))
            for s in items:
                dec.push(s)
            hyps = [{"text": "".join(w), "ngram": round(sc, 5)} for w, sc in dec.nbest(TOP)]
            label = next((k for k, h in enumerate(hyps) if accept_variants(gold, h["text"]) == h["text"]), None)
            if label is None or len(hyps) < 2:
                continue
            kept += 1
            top1 += label == 0
            out.write(json.dumps({"src": src, "context": context, "gold": gold, "hyps": hyps, "label": label},
                                 ensure_ascii=False) + "\n")
    tmp.replace(path)
    return path.name, len(rows), kept, top1


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("config", type=Path)
    ap.add_argument("data_dir", type=Path)
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--sources", default=DEFAULT_SOURCES)
    ap.add_argument("--exclude", nargs="*", default=[str(ROOT / "eval/testset/testset.tsv"),
                                                      str(ROOT / "eval/devset/moztw_dev.tsv")])
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--heldout", action="store_true", help="only the sentences the character model never saw")
    args = ap.parse_args()
    exclude = []
    for f in args.exclude:
        with open(f, encoding="utf-8", newline="") as fh:
            exclude += [r["text"] for r in csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE)]
    sources = {k: int(v) for k, v in (s.split("=") for s in args.sources.split(","))}
    rows = sample(args.data_dir, sources, exclude, args.seed, args.heldout)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    jobs = []
    for n in range(0, len(rows), SHARD):
        path = args.out_dir / f"shard-{n // SHARD:04d}.jsonl"
        if not path.exists():
            jobs.append((path, rows[n:n + SHARD]))
    print(f"{len(rows)} sentences, {len(jobs)} shards to do", flush=True)
    with Pool(args.workers, initializer=_init, initargs=(str(args.config.resolve()),)) as pool:
        for name, n, kept, top1 in pool.imap_unordered(_shard, jobs):
            print(f"{name}: {n} sentences, {kept} kept, n-gram right first in {top1}", flush=True)
    args.config.resolve().with_name(".rerank-data-config.json").unlink(missing_ok=True)


if __name__ == "__main__":
    main()
