"""Compare ways of re-ranking the decoder's results offline (data from dump_rerank.py).

Usage (from prototype/):
    uv run python ../eval/tools/rerank_sim.py RERANK_JSONL

A sentence counts as right as in the evaluation (the other standard form of a
character is accepted). The development set is split in two by id parity:
settings are chosen on the even half and reported on the odd half, so a
setting that only fits the noise does not look better than it is.

Methods, for each character model in the file:
  linear     ngram + w * charlm                       (the engine's way; w = 0.5 now)
  per-char   ngram + w * charlm / characters          (charlm as an average per character)
  gated      linear, but only when the n-gram's best two are within g of each other
  alone      charlm only (how good the model is by itself)
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "prototype"))
from bopogram.evaluate import accept_variants  # noqa: E402

WEIGHTS = [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 1.0, 1.2, 1.5, 2.0, 3.0, 5.0]
GATES = [0.5, 1.0, 2.0, 3.0, 5.0, 8.0, 1e9]


def right(gold: str, hyp: str) -> bool:
    return accept_variants(gold, hyp) == hyp


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("dump", type=Path)
    args = ap.parse_args()
    rows = [json.loads(line) for line in open(args.dump, encoding="utf-8")]
    for r in rows:
        r["ok"] = [right(r["gold"], h["text"]) for h in r["hyps"]]
        r["half"] = int("".join(c for c in r["id"] if c.isdigit()) or 0) % 2
    halves = [[r for r in rows if r["half"] == k] for k in (0, 1)]
    models = sorted(rows[0]["charlm"])

    def acc(rs, pick) -> float:
        return sum(r["ok"][pick(r)] for r in rs) / len(rs)

    def argmax(vals):
        return max(range(len(vals)), key=vals.__getitem__)

    print(f"{len(rows)} sentences ({len(halves[0])} to choose settings, {len(halves[1])} to report)")
    print(f"n-gram alone: {acc(halves[1], lambda r: 0):.2%}    "
          f"right answer among the candidates: {sum(any(r['ok']) for r in halves[1]) / len(halves[1]):.2%}")
    for m in models:
        def linear(w):
            return lambda r: argmax([h["ngram"] + w * c for h, c in zip(r["hyps"], r["charlm"][m])])

        def per_char(w):
            return lambda r: argmax([h["ngram"] + w * c / max(1, len(h["text"]))
                                     for h, c in zip(r["hyps"], r["charlm"][m])])

        def gated(w, g):
            def pick(r):
                h = r["hyps"]
                if len(h) < 2 or h[0]["ngram"] - max(x["ngram"] for x in h[1:]) > g:
                    return 0
                return linear(w)(r)
            return pick

        def best(make, grid):
            return max(grid, key=lambda p: acc(halves[0], make(*p)))

        print(f"\n== {m}")
        print(f"  alone:               {acc(halves[1], lambda r: argmax(r['charlm'][m])):.2%}")
        print(f"  linear w=0.5 (now):  {acc(halves[1], linear(0.5)):.2%}")
        (w,) = best(linear, [(w,) for w in WEIGHTS])
        print(f"  linear best w={w}:   {acc(halves[1], linear(w)):.2%}")
        (w,) = best(per_char, [(w,) for w in WEIGHTS + [8.0, 12.0, 20.0]])
        print(f"  per-char best w={w}: {acc(halves[1], per_char(w)):.2%}")
        w, g = best(gated, [(w, g) for w in WEIGHTS for g in GATES])
        print(f"  gated best w={w} g={g}: {acc(halves[1], gated(w, g)):.2%}")
        fixed = sum(r["ok"][linear(0.5)(r)] and not r["ok"][0] for r in halves[1])
        broke = sum(r["ok"][0] and not r["ok"][linear(0.5)(r)] for r in halves[1])
        print(f"  w=0.5 vs n-gram alone: {fixed} fixed, {broke} broken")


if __name__ == "__main__":
    main()
