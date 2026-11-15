"""Train the position-wise bopomofo -> character model.

Usage:
    uv run python neural/train.py DATA_DIR RUN_DIR [--steps N] [--tokens 12000] [--d-model 384] ...

DATA_DIR is the output of make_data.py. RUN_DIR receives:
    last.pt           checkpoint (model, optimizer, step); written atomically
    log.tsv           step, loss, learning rate, validation accuracy
The run resumes from last.pt when it exists.

Loss: cross-entropy over the characters each syllable may stand for (the
dictionary's candidates), at the buffer positions only. With tones withheld
the candidate set is the union over all tones of the syllable.
"""

from __future__ import annotations

import argparse
import math
import os
import sys
import time
from pathlib import Path

import numpy as np
import torch
import torch.nn.functional as F

sys.path.insert(0, str(Path(__file__).resolve().parent))
from data import Batches, Corpus, collate  # noqa: E402
from model import TONE_UNKNOWN, Config, Converter, Tables  # noqa: E402


def prepare(batch, tables: Tables, device):
    """Batch from data.collate -> model inputs, targets and the candidate mask rows."""
    chars, syls, lengths, drop = (t.to(device, non_blocking=True) for t in batch)
    is_syl = syls > 0
    bases = tables.syl_base[syls]
    tones = torch.where(drop, torch.full_like(syls, TONE_UNKNOWN), tables.syl_tone[syls])
    inp = torch.where(is_syl, torch.full_like(chars, tables.mask_char), chars)
    return inp, bases, tones, lengths, is_syl, chars, syls, drop


def masked_logits(logits, sel_syls, sel_bases, sel_drop, tables: Tables):
    allowed = torch.where(sel_drop[:, None], tables.allowed_base[sel_bases], tables.allowed[sel_syls])
    return logits.masked_fill(~allowed, float("-inf"))


@torch.no_grad()
def validate(model, tables, corpus: Corpus, device, tone_drop_all: bool = False) -> dict[str, float]:
    """Character accuracy on held-out sentences: the longest readable run is the buffer."""
    model.eval()
    hit: dict[str, list[int]] = {}
    items, names = [], []

    def flush():
        if not items:
            return
        batch = collate(items, None, 0.0)
        inp, bases, tones, lengths, is_syl, chars, syls, drop = prepare(batch, tables, device)
        if tone_drop_all:
            drop = is_syl.clone()
            tones = torch.where(drop, torch.full_like(tones, TONE_UNKNOWN), tones)
        with torch.autocast("cuda", dtype=torch.bfloat16, enabled=device.type == "cuda"):
            logits = model.logits(model(inp, bases, tones, lengths)[is_syl])
        pred = masked_logits(logits.float(), syls[is_syl], bases[is_syl], drop[is_syl], tables).argmax(-1)
        ok = (pred == chars[is_syl]).cpu().numpy()
        rows = is_syl.nonzero()[:, 0].cpu().numpy()
        for name, n_ok, n_all in zip(names, np.bincount(rows, weights=ok, minlength=len(items)),
                                     np.bincount(rows, minlength=len(items))):
            h = hit.setdefault(name, [0, 0])
            h[0] += int(n_ok)
            h[1] += int(n_all)
        items.clear()
        names.clear()

    for name, chars, syls in corpus.heldout():
        # Buffer = the readable run that ends the first clause of at least 4 characters.
        runs, start = [], None
        for i, s in enumerate(list(syls) + [0]):
            if s and start is None:
                start = i
            elif not s and start is not None:
                runs.append((start, i))
                start = None
        runs = [r for r in runs if r[1] - r[0] >= 4]
        if not runs:
            continue
        a, b = runs[min(1, len(runs) - 1)]
        a = max(a, b - 32)
        lo = max(0, a - 40)
        items.append((chars[lo:b], syls[lo:b], a - lo))
        names.append(name)
        if len(items) == 256:
            flush()
    flush()
    model.train()
    out = {k: v[0] / max(1, v[1]) for k, v in hit.items()}
    out["all"] = sum(v[0] for v in hit.values()) / max(1, sum(v[1] for v in hit.values()))
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("data_dir", type=Path)
    ap.add_argument("run_dir", type=Path)
    ap.add_argument("--steps", type=int, default=20000)
    ap.add_argument("--tokens", type=int, default=12000, help="padded positions per batch")
    ap.add_argument("--lr", type=float, default=6e-4)
    ap.add_argument("--warmup", type=int, default=2000)
    ap.add_argument("--d-model", type=int, default=384)
    ap.add_argument("--layers", type=int, default=6)
    ap.add_argument("--heads", type=int, default=6)
    ap.add_argument("--tone-drop", type=float, default=0.15)
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--save-every", type=int, default=1000)
    ap.add_argument("--eval-every", type=int, default=5000)
    ap.add_argument("--seed", type=int, default=1)
    args = ap.parse_args()

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    torch.backends.cuda.matmul.allow_tf32 = True
    torch.backends.cudnn.allow_tf32 = True
    args.run_dir.mkdir(parents=True, exist_ok=True)
    corpus = Corpus(args.data_dir)
    tables = Tables(corpus.vocab)
    for name in ("syl_base", "syl_tone", "allowed", "allowed_base"):
        setattr(tables, name, getattr(tables, name).to(device))
    cfg = Config(n_chars=len(tables.chars), n_bases=len(tables.bases), d_model=args.d_model,
                 n_layers=args.layers, n_heads=args.heads, d_ff=4 * args.d_model)
    model = Converter(cfg).to(device)
    decay = [p for n, p in model.named_parameters() if p.dim() >= 2]
    no_decay = [p for n, p in model.named_parameters() if p.dim() < 2]
    opt = torch.optim.AdamW([{"params": decay, "weight_decay": 0.01}, {"params": no_decay, "weight_decay": 0.0}],
                            lr=args.lr, betas=(0.9, 0.98), fused=device.type == "cuda")
    step = 0
    ckpt_path = args.run_dir / "last.pt"
    if ckpt_path.exists():
        ckpt = torch.load(ckpt_path, map_location=device, weights_only=False)
        model.load_state_dict(ckpt["model"])
        opt.load_state_dict(ckpt["opt"])
        step = ckpt["step"]
        print(f"resumed from step {step}", flush=True)
    print(f"{model.n_params() / 1e6:.1f}M parameters, device {device}", flush=True)

    def lr_at(s: int) -> float:
        if s < args.warmup:
            return args.lr * (s + 1) / args.warmup
        t = (s - args.warmup) / max(1, args.steps - args.warmup)
        return args.lr * (0.05 + 0.95 * 0.5 * (1 + math.cos(math.pi * min(1.0, t))))

    def save() -> None:
        tmp = ckpt_path.with_suffix(".tmp")
        torch.save({"model": model.state_dict(), "opt": opt.state_dict(), "step": step,
                    "config": model.config_dict(), "args": vars(args) | {"data_dir": str(args.data_dir),
                                                                         "run_dir": str(args.run_dir)}}, tmp)
        os.replace(tmp, ckpt_path)

    # A different seed after each resume, so a resumed run does not replay the same batches.
    loader = torch.utils.data.DataLoader(
        Batches(args.data_dir, args.tokens, args.tone_drop, args.seed + step), batch_size=None,
        num_workers=args.workers, pin_memory=True, prefetch_factor=4, persistent_workers=True)
    log = open(args.run_dir / "log.tsv", "a", encoding="utf-8")
    model.train()
    t0, seen, run_loss, run_n = time.time(), 0, 0.0, 0
    for batch in loader:
        if step >= args.steps:
            break
        for g in opt.param_groups:
            g["lr"] = lr_at(step)
        inp, bases, tones, lengths, is_syl, chars, syls, drop = prepare(batch, tables, device)
        with torch.autocast("cuda", dtype=torch.bfloat16, enabled=device.type == "cuda"):
            logits = model.logits(model(inp, bases, tones, lengths)[is_syl])
        sel = masked_logits(logits.float(), syls[is_syl], bases[is_syl], drop[is_syl], tables)
        loss = F.cross_entropy(sel, chars[is_syl])
        opt.zero_grad(set_to_none=True)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        opt.step()
        step += 1
        seen += int(is_syl.sum())
        run_loss += float(loss)
        run_n += 1
        if step % 100 == 0:
            rate = seen / (time.time() - t0)
            line = f"{step}\t{run_loss / run_n:.4f}\t{lr_at(step):.2e}"
            print(f"step {line}\t{rate / 1000:.1f}k syll/s", flush=True)
            log.write(line + "\n")
            log.flush()
            run_loss, run_n = 0.0, 0
        if step % args.save_every == 0 or step == args.steps:
            save()
        if step % args.eval_every == 0 or step == args.steps:
            acc = validate(model, tables, corpus, device)
            toneless = validate(model, tables, corpus, device, tone_drop_all=True)["all"]
            msg = " ".join(f"{k}={v:.4f}" for k, v in acc.items())
            print(f"validation at step {step}: {msg} toneless={toneless:.4f}", flush=True)
            log.write(f"{step}\tval\t{msg} toneless={toneless:.4f}\n")
            log.flush()
    save()


if __name__ == "__main__":
    main()
