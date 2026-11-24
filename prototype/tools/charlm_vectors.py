"""Test vectors for another implementation of the character model in the
engine's own format (prototype/docs/charlm-native.md).

Usage (WSL):
    uv run python tools/charlm_vectors.py MODEL_DIR TESTSET OUT_JSONL [--limit 300]

MODEL_DIR holds charlm.bin and charlm.json. Each output line is one call of the
re-ranker, made from a test sentence: its context, and the sentence with a few
variants (two characters swapped, one dropped, a full stop added, cut in half):

    {"context": "...", "texts": [...], "seqs": [[ids], ...], "firsts": [...],
     "logp": [...], "fresh": true}

`seqs` and `firsts` are what charlm.py builds from context and texts, so the
encoding can be checked separately from the arithmetic. `logp` is computed
from scratch for every sequence (no prefix cache). The first line also has
"trace": the float32 hidden vector after each layer for its first sequence,
rounded to 6 digits, to find where two implementations part.
"""

from __future__ import annotations

import argparse
import csv
import json
import random
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from bopogram.charlm import MAX_CONTEXT, CharLMScorer  # noqa: E402
from bopogram.charlm_native import NativeCharLM  # noqa: E402


def variants(text: str, rng: random.Random) -> list[str]:
    out = [text]
    if len(text) > 2:
        i = rng.randrange(len(text) - 1)
        out.append(text[:i] + text[i + 1] + text[i] + text[i + 2:])
        out.append(text[:i] + text[i + 1:])
    out.append(text + "。")
    out.append(text[: max(1, len(text) // 2)])
    return list(dict.fromkeys(out))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("model_dir", type=Path)
    ap.add_argument("testset", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--limit", type=int, default=300)
    args = ap.parse_args()
    path = args.model_dir / "charlm.bin"
    scorer = CharLMScorer(path)
    rng = random.Random(0)
    with open(args.testset, encoding="utf-8", newline="") as fh:
        rows = list(csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE))[: args.limit]
    with open(args.out, "w", encoding="utf-8") as out:
        for n, row in enumerate(rows):
            context = (row.get("context") or "")[-MAX_CONTEXT:]
            texts = variants(row["text"], rng)
            prefix = [scorer._bos] + scorer.encode(context)
            tail = [scorer._bos] if scorer._eos else []
            seqs, firsts, logp = [], [], []
            for t in texts:
                body = scorer.encode(t) + tail
                seq = (prefix + body)[-scorer._max_len:]
                first = max(0, len(seq) - len(body) - 1)
                seqs.append(seq)
                firsts.append(first)
                logp.append(NativeCharLM(path).logp(seq, first))
            rec = {"context": context, "texts": texts, "seqs": seqs, "firsts": firsts, "logp": logp, "fresh": True}
            if n == 0:
                rec["trace"] = trace(NativeCharLM(path), seqs[0])
            out.write(json.dumps(rec, ensure_ascii=False) + "\n")
    print(f"{len(rows)} calls written to {args.out}")


def trace(model: NativeCharLM, seq: list[int]) -> list[list[list[float]]]:
    """Hidden vectors after the embedding and after each block, for every position."""
    from bopogram import charlm_native as cn

    d, h = model.d, model.h
    dh = d // h
    x = model.emb[np.array(seq)] + model.pos[: len(seq)]
    out = [np.round(x.astype(np.float64), 6).tolist()]
    n = len(seq)
    visible = np.arange(n)[None, :] <= np.arange(n)[:, None]
    for blk in model.blocks:
        qkv = blk["qkv"](cn.layer_norm(x, *blk["n1"], model.eps))
        q, k, v = qkv[:, :d], qkv[:, d:2 * d], qkv[:, 2 * d:]
        a = np.empty((n, d), dtype=np.float32)
        for hd in range(h):
            sl = slice(hd * dh, (hd + 1) * dh)
            a[:, sl] = cn.attention(q[:, sl], k[:, sl], v[:, sl], visible, dh)
        x = x + blk["proj"](a)
        x = x + blk["ff2"](cn.gelu(blk["ff1"](cn.layer_norm(x, *blk["n2"], model.eps))))
        out.append(np.round(x.astype(np.float64), 6).tolist())
    return out


if __name__ == "__main__":
    main()
