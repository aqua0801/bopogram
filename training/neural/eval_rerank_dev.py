"""Score the development set's re-ranking candidates with character-model
checkpoints on the GPU, and report how often the engine's choice is right.

Usage (from training/):
    uv run python neural/eval_rerank_dev.py RERANK_JSONL CKPT [CKPT ...] [--weight 0.5]

RERANK_JSONL is eval/tools/dump_rerank.py's output (the n-gram decoder's
candidates and scores for every development sentence). The checkpoints run in
PyTorch (bfloat16), not in the engine's 8-bit format, so the numbers are for
comparing checkpoints with each other; the final figures come from the engine.
A sentence is right as in the evaluation (the other standard form of a
character is accepted). Also reported: sentences the re-ranking fixed and broke
compared with the n-gram's first choice.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import torch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "prototype"))
from bopogram.evaluate import accept_variants  # noqa: E402
from charlm import BOS, load_lm  # noqa: E402
from finetune_rerank import candidate_scores  # noqa: E402


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("dump", type=Path)
    ap.add_argument("ckpts", type=Path, nargs="+")
    ap.add_argument("--weight", type=float, nargs="+", default=[0.5])
    ap.add_argument("--top", type=int, nargs="+", default=[0], help="re-rank only the first N results (0: all in the file)")
    args = ap.parse_args()
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    rows = [json.loads(line) for line in open(args.dump, encoding="utf-8")]
    for r in rows:
        r["ok"] = [accept_variants(r["gold"], h["text"]) == h["text"] for h in r["hyps"]]
    base = sum(r["ok"][0] for r in rows)
    print(f"{len(rows)} sentences; n-gram alone {base / len(rows):.2%}")
    for path in args.ckpts:
        model, step = load_lm(path, device)
        ck = torch.load(path, map_location="cpu", weights_only=False)
        chars = ck["chars"]
        cid, bos, eos = {c: i for i, c in enumerate(chars)}, chars.index(BOS), bool(ck.get("eos"))
        model.eval()
        scores = []
        with torch.no_grad():
            for k in range(0, len(rows), 64):
                part = rows[k: k + 64]
                sc, _, _ = candidate_scores(model, part, cid, bos, eos, model.cfg.max_len, device)
                scores += [s.float().cpu() for s in sc]
        for top in args.top:
            for w in args.weight:
                right = fixed = broke = 0
                for r, m in zip(rows, scores):
                    k = top or len(r["hyps"])
                    z = torch.tensor([h["ngram"] for h in r["hyps"][:k]]) + w * m[:k]
                    pick = int(torch.argmax(z))
                    right += r["ok"][pick]
                    fixed += r["ok"][pick] and not r["ok"][0]
                    broke += r["ok"][0] and not r["ok"][pick]
                print(f"{path.parent.name}/{path.name} (step {step}) top={top or 'all'} w={w}: "
                      f"{right / len(rows):.2%}  fixed {fixed}, broken {broke}", flush=True)


if __name__ == "__main__":
    main()
