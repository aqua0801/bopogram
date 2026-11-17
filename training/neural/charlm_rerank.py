"""Re-rank the n-gram engine's candidate sentences with the character language model.

Usage:
    uv run python neural/charlm_rerank.py CHECKPOINT DEV_NBEST TEST_NBEST [--top 5 10 20]

The n-best files come from `bopogram eval --dump-nbest FILE --beam 100 --nbest N`.
Each candidate gets   n-gram score + weight * ln P_charlm(sentence | left context).
The weight is chosen on the development set and then applied to the test set.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

import torch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from charlm import BOS, MAX_LEN, load_lm  # noqa: E402

MAX_CONTEXT = 40
WEIGHTS = [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.8, 1.0, 1.3, 1.6, 2.0]
_PRONOUNS = str.maketrans({"她": "他", "它": "他", "牠": "他", "祂": "他"})


def encode(text: str, cid: dict[str, int]) -> list[int]:
    """Text -> character ids the way make_data.py encodes the corpus."""
    out, run = [], None
    for ch in text:
        kind = "<n>" if ch.isdigit() else "<e>" if ch.isascii() and ch.isalpha() else None
        if kind:
            if run != kind:
                out.append(cid[kind])
            run = kind
            continue
        run = None
        if not ch.isspace():
            out.append(cid.get(ch, cid["<x>"] if "一" <= ch <= "鿿" else cid["<p>"]))
    return out


@torch.no_grad()
def score_file(model, cid, path: Path, top: int, device) -> tuple[list[dict], float]:
    rows, spent = [], 0.0
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            rec = json.loads(line)
            hyps = rec["hyps"][:top]
            prefix = [cid[BOS]] + encode(rec["context"][-MAX_CONTEXT:], cid)
            seqs = [(prefix + encode(text, cid))[-MAX_LEN:] for text, _ in hyps]
            n_pre = len(seqs[0]) - len(hyps[0][0])
            t0 = time.perf_counter()
            batch = torch.tensor(seqs, device=device)
            logp = torch.log_softmax(model(batch[:, :-1]).float(), dim=-1)
            tok = logp.gather(2, batch[:, 1:, None])[:, :, 0]
            lm = tok[:, max(0, n_pre - 1):].sum(1).tolist()
            spent += time.perf_counter() - t0
            rows.append({"id": rec["id"], "category": rec["category"], "gold": rec["gold"],
                         "hyps": [(text, s, l) for (text, s), l in zip(hyps, lm)]})
    return rows, spent / max(1, len(rows))


def accuracy(rows: list[dict], weight: float) -> dict[str, list[int]]:
    out: dict[str, list[int]] = {}
    for r in rows:
        best = max(r["hyps"], key=lambda h: h[1] + weight * h[2])[0]
        for g in ("all", r["category"]):
            st = out.setdefault(g, [0, 0, 0])
            st[0] += 1
            st[1] += best == r["gold"]
            st[2] += best.translate(_PRONOUNS) == r["gold"].translate(_PRONOUNS)
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("checkpoint", type=Path)
    ap.add_argument("dev", type=Path)
    ap.add_argument("test", type=Path)
    ap.add_argument("--top", type=int, nargs="+", default=[5, 10, 20])
    ap.add_argument("--cpu", action="store_true")
    ap.add_argument("--changes", type=Path, help="write test sentences whose result changed")
    args = ap.parse_args()

    device = torch.device("cpu" if args.cpu or not torch.cuda.is_available() else "cuda")
    model, step = load_lm(args.checkpoint, device)
    ckpt = torch.load(args.checkpoint, map_location="cpu", weights_only=False)
    cid = {c: i for i, c in enumerate(ckpt["chars"])}
    print(f"character LM at step {step}, {model.n_params() / 1e6:.1f}M parameters, {device}")
    for top in args.top:
        dev, _ = score_file(model, cid, args.dev, top, device)
        test, per = score_file(model, cid, args.test, top, device)
        dev_acc = {w: accuracy(dev, w)["all"] for w in WEIGHTS}
        w = max(WEIGHTS, key=lambda x: dev_acc[x][1])
        print(f"\ntop {top}: weight {w} (dev {dev_acc[0.0][1] / dev_acc[0.0][0]:.1%} -> "
              f"{dev_acc[w][1] / dev_acc[w][0]:.1%}), {per * 1000:.1f} ms per sentence")
        base, new = accuracy(test, 0.0), accuracy(test, w)
        for g in sorted(base, key=lambda k: (k != "all", k)):
            n = base[g][0]
            print(f"  {g:<10}{n:>5}  {base[g][1] / n:6.1%} -> {new[g][1] / n:6.1%}   "
                  f"pronouns merged {base[g][2] / n:6.1%} -> {new[g][2] / n:6.1%}")
        if args.changes and top == args.top[-1]:
            with open(args.changes, "w", encoding="utf-8") as fh:
                for r in test:
                    a = max(r["hyps"], key=lambda h: h[1])[0]
                    b = max(r["hyps"], key=lambda h: h[1] + w * h[2])[0]
                    if a != b:
                        kind = "fixed" if b == r["gold"] else "broken" if a == r["gold"] else "other"
                        fh.write(f"{kind}\t{r['id']}\t{r['gold']}\t{a}\t{b}\n")


if __name__ == "__main__":
    main()
