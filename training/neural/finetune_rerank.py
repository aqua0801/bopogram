"""Fine-tune the character model to choose among the n-gram decoder's results
(prototype/docs/charlm-rerank-training.md).

Usage (from training/):
    uv run python neural/finetune_rerank.py INIT_CKPT DATA_DIR RUN_DIR --val HELDOUT_DIR
        [--steps 6000] [--batch 32] [--lr 5e-5] [--weight 0.5] [--lm 1.0]

DATA_DIR holds make_rerank_data.py's shards; --val its --heldout output, for
validation (the starting model never saw those sentences, so it has no unfair advantage). For each example the model scores every candidate exactly as the
engine does (bopogram.charlm.sequence: <s>, the context's last 40 characters,
the candidate, end of line), and the engine's choice is the highest
    z_i = ngram_i + weight * charlm_i.
Loss = cross-entropy of the right candidate under softmax(z), plus `lm` times
the usual next-character loss on the right sentence, which keeps the model a
language model (its scores stay on the same scale).

RUN_DIR receives last.pt (with the optimizer, for resuming; written
atomically), stepN.pt at every evaluation, and log.tsv. Validation reports how
often the engine's choice is right with the current weights, next to the n-gram
alone and the starting model.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import random
import sys
import time
from dataclasses import asdict
from pathlib import Path

import torch
import torch.nn.functional as F

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "prototype"))
from bopogram.charlm import sequence  # noqa: E402
from charlm import BOS, load_lm  # noqa: E402


DE = set("的地得")


def de_only(gold: str, text: str) -> bool:
    """`text` differs from `gold` only where both have one of 的 / 地 / 得."""
    return len(gold) == len(text) and gold != text and all(
        a == b or (a in DE and b in DE) for a, b in zip(gold, text))


def read(data_dir: Path) -> list[dict]:
    return [json.loads(line) for p in sorted(data_dir.glob("shard-*.jsonl")) for line in open(p, encoding="utf-8")]


def candidate_scores(model, examples, cid, bos, eos, max_len, device) -> list[torch.Tensor]:
    """charlm score of every candidate of every example (as the engine computes it)."""
    seqs, firsts, owner = [], [], []
    for k, ex in enumerate(examples):
        for h in ex["hyps"]:
            s, f = sequence(ex["context"], h["text"], cid, bos, eos, max_len)
            seqs.append(s)
            firsts.append(f)
            owner.append(k)
    n = max(len(s) for s in seqs)
    ids = torch.zeros((len(seqs), n), dtype=torch.long)
    mask = torch.zeros((len(seqs), n - 1))
    for i, (s, f) in enumerate(zip(seqs, firsts)):
        ids[i, : len(s)] = torch.tensor(s)
        mask[i, f: len(s) - 1] = 1
    ids, mask = ids.to(device), mask.to(device)
    with torch.autocast("cuda", dtype=torch.bfloat16, enabled=device.type == "cuda"):
        logits = model(ids[:, :-1])
    logp = torch.log_softmax(logits.float(), dim=-1).gather(2, ids[:, 1:, None])[:, :, 0]
    totals = (logp * mask).sum(1)
    out, i = [], 0
    for ex in examples:
        m = len(ex["hyps"])
        out.append(totals[i: i + m])
        i += m
    return out, ids, mask


def lm_loss(model, examples, cid, bos, eos, max_len, device) -> torch.Tensor:
    seqs = [sequence(ex["context"], ex["gold"], cid, bos, eos, max_len)[0] for ex in examples]
    n = max(len(s) for s in seqs)
    ids = torch.zeros((len(seqs), n), dtype=torch.long)
    for i, s in enumerate(seqs):
        ids[i, : len(s)] = torch.tensor(s)
    ids = ids.to(device)
    with torch.autocast("cuda", dtype=torch.bfloat16, enabled=device.type == "cuda"):
        logits = model(ids[:, :-1])
    return F.cross_entropy(logits.float().flatten(0, 1), ids[:, 1:].flatten(), ignore_index=0)


@torch.no_grad()
def validate(model, examples, cid, bos, eos, max_len, device, weight) -> float:
    model.eval()
    right = 0
    for k in range(0, len(examples), 64):
        part = examples[k: k + 64]
        scores, _, _ = candidate_scores(model, part, cid, bos, eos, max_len, device)
        for ex, m in zip(part, scores):
            z = torch.tensor([h["ngram"] for h in ex["hyps"]], device=device) + weight * m
            right += int(torch.argmax(z)) == ex["label"]
    model.train()
    return right / len(examples)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("init", type=Path)
    ap.add_argument("data_dir", type=Path)
    ap.add_argument("run_dir", type=Path)
    ap.add_argument("--val", type=Path, required=True,
                    help="make_rerank_data.py --heldout output: sentences the character model never saw")
    ap.add_argument("--steps", type=int, default=6000)
    ap.add_argument("--batch", type=int, default=32, help="examples per step")
    ap.add_argument("--lr", type=float, default=5e-5)
    ap.add_argument("--warmup", type=int, default=200)
    ap.add_argument("--weight", type=float, default=0.5, help="the engine's re-ranking weight")
    ap.add_argument("--lm", type=float, default=1.0, help="weight of the next-character loss")
    ap.add_argument("--eval-every", type=int, default=1000)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--drop-de", action="store_true",
                    help="leave out examples where a candidate differs from the right one only in 的/地/得: "
                         "blogs write 的 for all three, and the model should not learn that")
    args = ap.parse_args()
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    torch.manual_seed(args.seed)
    model, _ = load_lm(args.init, device)
    init = torch.load(args.init, map_location="cpu", weights_only=False)
    chars, eos = init["chars"], bool(init.get("eos"))
    cid = {c: i for i, c in enumerate(chars)}
    bos, max_len = chars.index(BOS), model.cfg.max_len
    train, val = read(args.data_dir), read(args.val)
    if args.drop_de:
        n = len(train)
        train = [ex for ex in train if not any(de_only(ex["gold"], h["text"]) for h in ex["hyps"])]
        print(f"--drop-de: {n - len(train)} examples left out", flush=True)
    print(f"{len(train)} training examples, {len(val)} for validation "
          f"({sum(ex['label'] != 0 for ex in train)} where the n-gram's first is wrong)", flush=True)
    opt = torch.optim.AdamW(model.parameters(), lr=args.lr, weight_decay=0.01, betas=(0.9, 0.98))
    args.run_dir.mkdir(parents=True, exist_ok=True)
    ckpt_path, step = args.run_dir / "last.pt", 0
    if ckpt_path.exists():
        ck = torch.load(ckpt_path, map_location=device, weights_only=False)
        model.load_state_dict(ck["model"])
        opt.load_state_dict(ck["opt"])
        step = ck["step"]
        print(f"resumed from step {step}", flush=True)
    log = open(args.run_dir / "log.tsv", "a", encoding="utf-8")
    if step == 0:
        ngram_only = sum(ex["label"] == 0 for ex in val) / len(val)
        start = validate(model, val, cid, bos, eos, max_len, device, args.weight)
        msg = f"validation: n-gram alone {ngram_only:.2%}, starting model {start:.2%}"
        print(msg, flush=True)
        log.write(f"0\tval\t{msg}\n")

    def save(path: Path, with_opt: bool) -> None:
        data = {"model": model.state_dict(), "step": step, "config": asdict(model.cfg), "chars": chars, "eos": eos}
        if with_opt:
            data["opt"] = opt.state_dict()
        tmp = path.with_suffix(".tmp")
        torch.save(data, tmp)
        os.replace(tmp, path)

    rng = random.Random(args.seed + step)
    model.train()
    t0, run_rank, run_lm, run_n = time.time(), 0.0, 0.0, 0
    while step < args.steps:
        batch = rng.sample(train, args.batch)
        lr = args.lr * min(1.0, (step + 1) / args.warmup) * (0.1 + 0.9 * 0.5 * (1 + math.cos(math.pi * step / args.steps)))
        for g in opt.param_groups:
            g["lr"] = lr
        scores, _, _ = candidate_scores(model, batch, cid, bos, eos, max_len, device)
        rank = torch.stack([
            F.cross_entropy((torch.tensor([h["ngram"] for h in ex["hyps"]], device=device) + args.weight * m)[None],
                            torch.tensor([ex["label"]], device=device))
            for ex, m in zip(batch, scores)]).mean()
        lm = lm_loss(model, batch, cid, bos, eos, max_len, device) if args.lm else torch.zeros((), device=device)
        loss = rank + args.lm * lm
        opt.zero_grad(set_to_none=True)
        loss.backward()
        norm = float(torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0))
        if not math.isfinite(norm) or not math.isfinite(float(loss)):
            opt.zero_grad(set_to_none=True)
            continue
        opt.step()
        step += 1
        run_rank += float(rank)
        run_lm += float(lm)
        run_n += 1
        if step % 100 == 0:
            line = f"{step}\t{run_rank / run_n:.4f}\t{run_lm / run_n:.4f}\t{lr:.2e}"
            print(f"step {line}\t{(time.time() - t0) / step:.2f} s/step", flush=True)
            log.write(line + "\n")
            log.flush()
            run_rank, run_lm, run_n = 0.0, 0.0, 0
        if step % args.eval_every == 0 or step == args.steps:
            save(ckpt_path, True)
            save(args.run_dir / f"step{step}.pt", False)
            acc = validate(model, val, cid, bos, eos, max_len, device, args.weight)
            print(f"validation at step {step}: {acc:.2%}", flush=True)
            log.write(f"{step}\tval\t{acc:.4f}\n")
            log.flush()
    save(ckpt_path, True)


if __name__ == "__main__":
    main()
