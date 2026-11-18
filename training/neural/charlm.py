"""Character-level language model trained on plain text only (no readings).

Usage:
    uv run python neural/charlm.py DATA_DIR RUN_DIR [--steps N] [--d-model 384] [--layers 6] ...

A causal Transformer predicts every character of a sentence from the
characters before it. Unlike the position-wise converter (model.py) it never
sees a syllable, so nothing in its training data depends on automatically
assigned readings. At typing time the dictionary supplies the candidates and
this model only says which candidate sentence reads best (charlm_rerank.py).

DATA_DIR is the output of make_data.py; only the character arrays are used.
RUN_DIR receives last.pt (written atomically), stepN.pt at every evaluation and log.tsv; the run resumes
from last.pt when it exists.
"""

from __future__ import annotations

import argparse
import math
import os
import sys
import time
from dataclasses import asdict, dataclass
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F

sys.path.insert(0, str(Path(__file__).resolve().parent))
from data import Corpus  # noqa: E402
from model import Block  # noqa: E402

MAX_LEN = 128
BOS = "<s>"


@dataclass
class LMConfig:
    n_chars: int
    d_model: int = 384
    n_layers: int = 6
    n_heads: int = 6
    d_ff: int = 1536
    max_len: int = MAX_LEN
    dropout: float = 0.1


class CharLM(nn.Module):
    def __init__(self, cfg: LMConfig):
        super().__init__()
        self.cfg = cfg
        self.char_emb = nn.Embedding(cfg.n_chars, cfg.d_model)
        self.pos_emb = nn.Embedding(cfg.max_len, cfg.d_model)
        self.drop = nn.Dropout(cfg.dropout)
        self.blocks = nn.ModuleList(Block(cfg) for _ in range(cfg.n_layers))
        self.norm = nn.LayerNorm(cfg.d_model)
        self.out_bias = nn.Parameter(torch.zeros(cfg.n_chars))
        self.apply(self._init)

    @staticmethod
    def _init(m: nn.Module) -> None:
        if isinstance(m, (nn.Linear, nn.Embedding)):
            nn.init.normal_(m.weight, std=0.02)
            if isinstance(m, nn.Linear) and m.bias is not None:
                nn.init.zeros_(m.bias)

    def forward(self, chars: torch.Tensor) -> torch.Tensor:
        """chars [B, N] -> logits [B, N, n_chars]; logits[:, i] predict character i + 1.
        Padding sits on the right, so the causal mask alone keeps it out of sight."""
        n = chars.shape[1]
        x = self.drop(self.char_emb(chars) + self.pos_emb(torch.arange(n, device=chars.device)))
        # Additive mask (0 / -inf): same meaning as a boolean one, and exports to ONNX correctly.
        mask = torch.full((n, n), float("-inf"), device=chars.device).triu(1)
        for blk in self.blocks:
            x = blk(x, mask)
        return self.norm(x) @ self.char_emb.weight.T + self.out_bias

    def n_params(self) -> int:
        return sum(p.numel() for p in self.parameters())


def crop(chars: np.ndarray, bos: int, rng: np.random.Generator, eos: bool) -> np.ndarray:
    """One training sequence: usually the start of a line, sometimes text from the middle
    (at typing time the known text before the cursor may begin anywhere). With `eos` the
    line ends with <s> as well, so the model learns how sentences end."""
    tail = [bos] if eos else []
    if len(chars) > 8 and rng.random() < 0.3:
        start = int(rng.integers(1, len(chars) - 4))
        seq = np.concatenate((chars[start:], tail))
    else:
        seq = np.concatenate(([bos], chars, tail))
    return seq[:MAX_LEN].astype(np.int64)


class Batches(torch.utils.data.IterableDataset):
    """Endless stream of padded [B, N] batches; sequences of similar length share a batch."""

    POOL = 20000

    def __init__(self, data_dir: Path, max_tokens: int, seed: int, eos: bool, weights: dict[str, float]):
        self.data_dir, self.max_tokens, self.seed = data_dir, max_tokens, seed
        self.eos, self.source_weights = eos, weights

    def __iter__(self):
        info = torch.utils.data.get_worker_info()
        rng = np.random.default_rng([self.seed, info.id if info else 0])
        corpus = Corpus(self.data_dir)
        reweight(corpus, self.source_weights)
        bos = corpus.vocab["chars"].index(BOS)
        while True:
            items = []
            while len(items) < self.POOL:
                name = corpus.names[rng.choice(len(corpus.names), p=corpus.weights)]
                shards = corpus.sources[name]
                shard = shards[rng.choice(len(shards), p=corpus.shard_p[name])]
                chars, _ = corpus.sentence(shard, int(rng.integers(shard.lo, shard.n)))
                if len(chars) >= 2:
                    items.append(crop(chars, bos, rng, self.eos))
            items.sort(key=len)
            batches, cur = [], []
            for seq in items:
                if cur and (len(cur) + 1) * len(seq) > self.max_tokens:
                    batches.append(cur)
                    cur = []
                cur.append(seq)
            batches.append(cur)
            for i in rng.permutation(len(batches)):
                yield pad(batches[i])


def reweight(corpus: Corpus, weights: dict[str, float]) -> None:
    """Override the sampling share of some sources (others keep their default share)."""
    if weights:
        w = np.array([weights.get(n, corpus.weights[i]) for i, n in enumerate(corpus.names)])
        corpus.weights = w / w.sum()


def parse_weights(text: str) -> dict[str, float]:
    return {k: float(v) for k, v in (kv.split("=") for kv in text.split(",") if kv)}


def pad(seqs: list[np.ndarray]) -> torch.Tensor:
    out = np.zeros((len(seqs), max(len(s) for s in seqs)), dtype=np.int64)  # 0 = <pad>
    for i, s in enumerate(seqs):
        out[i, : len(s)] = s
    return torch.from_numpy(out)


def lm_loss(model: CharLM, batch: torch.Tensor, device, reduction: str = "mean") -> torch.Tensor:
    with torch.autocast("cuda", dtype=torch.bfloat16, enabled=device.type == "cuda"):
        logits = model(batch[:, :-1])
    return F.cross_entropy(logits.float().flatten(0, 1), batch[:, 1:].flatten(), ignore_index=0, reduction=reduction)


@torch.no_grad()
def validate(model: CharLM, corpus: Corpus, device, eos: bool) -> dict[str, float]:
    """Perplexity per character on the held-out sentences of every source."""
    model.eval()
    bos = corpus.vocab["chars"].index(BOS)
    nll: dict[str, list[float]] = {}
    by_src: dict[str, list[np.ndarray]] = {}
    for name, chars, _ in corpus.heldout():
        if len(chars) >= 2:
            seq = np.concatenate(([bos], chars, [bos] if eos else []))[:MAX_LEN]
            by_src.setdefault(name, []).append(seq.astype(np.int64))
    for name, seqs in by_src.items():
        seqs.sort(key=len)
        tot = nll.setdefault(name, [0.0, 0])
        for i in range(0, len(seqs), 128):
            batch = pad(seqs[i:i + 128]).to(device)
            tot[0] += float(lm_loss(model, batch, device, "sum"))
            tot[1] += int((batch[:, 1:] > 0).sum())
    model.train()
    out = {k: math.exp(v[0] / max(1, v[1])) for k, v in nll.items()}
    out["all"] = math.exp(sum(v[0] for v in nll.values()) / max(1, sum(v[1] for v in nll.values())))
    return out


def load_lm(ckpt_path: Path, device):
    ckpt = torch.load(ckpt_path, map_location=device, weights_only=False)
    model = CharLM(LMConfig(**ckpt["config"])).to(device)
    model.load_state_dict(ckpt["model"])
    model.eval()
    return model, ckpt["step"]


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
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--save-every", type=int, default=1000)
    ap.add_argument("--eval-every", type=int, default=5000)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--no-eos", dest="eos", action="store_false", help="do not mark the end of a line")
    ap.add_argument("--weights", type=parse_weights, default={},
                    help="source sampling shares, e.g. twllm=0.2,zhwiki=0.25")
    args = ap.parse_args()

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    torch.backends.cuda.matmul.allow_tf32 = True
    torch.backends.cudnn.allow_tf32 = True
    args.run_dir.mkdir(parents=True, exist_ok=True)
    corpus = Corpus(args.data_dir)
    reweight(corpus, args.weights)
    print("source shares:", " ".join(f"{n}={w:.2f}" for n, w in zip(corpus.names, corpus.weights)), flush=True)
    cfg = LMConfig(n_chars=len(corpus.vocab["chars"]), d_model=args.d_model, n_layers=args.layers,
                   n_heads=args.heads, d_ff=4 * args.d_model)
    model = CharLM(cfg).to(device)
    decay = [p for p in model.parameters() if p.dim() >= 2]
    no_decay = [p for p in model.parameters() if p.dim() < 2]
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
        torch.save({"model": model.state_dict(), "opt": opt.state_dict(), "step": step, "config": asdict(cfg),
                    "chars": corpus.vocab["chars"], "eos": args.eos}, tmp)
        os.replace(tmp, ckpt_path)

    loader = torch.utils.data.DataLoader(
        Batches(args.data_dir, args.tokens, args.seed + step, args.eos, args.weights), batch_size=None,
        num_workers=args.workers, pin_memory=True, prefetch_factor=4, persistent_workers=True)
    log = open(args.run_dir / "log.tsv", "a", encoding="utf-8")
    model.train()
    t0, seen, run_loss, run_n = time.time(), 0, 0.0, 0
    skipped, max_norm = 0, 0.0
    for batch in loader:
        if step >= args.steps:
            break
        for g in opt.param_groups:
            g["lr"] = lr_at(step)
        batch = batch.to(device, non_blocking=True)
        loss = lm_loss(model, batch, device)
        opt.zero_grad(set_to_none=True)
        loss.backward()
        norm = float(torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0))
        if not math.isfinite(norm) or not math.isfinite(float(loss)):
            skipped += 1  # one bad batch must not poison the optimizer state
            opt.zero_grad(set_to_none=True)
            continue
        max_norm = max(max_norm, norm)
        opt.step()
        step += 1
        seen += int((batch[:, 1:] > 0).sum())
        run_loss += float(loss)
        run_n += 1
        if step % 100 == 0:
            rate = seen / (time.time() - t0)
            line = f"{step}\t{run_loss / run_n:.4f}\t{lr_at(step):.2e}\t{max_norm:.1f}\t{skipped}"
            print(f"step {line}\t{rate / 1000:.1f}k char/s", flush=True)
            max_norm = 0.0
            log.write(line + "\n")
            log.flush()
            run_loss, run_n = 0.0, 0
        if step % args.save_every == 0 or step == args.steps:
            save()
        if step % args.eval_every == 0:
            # Numbered copies: a run that degrades later can fall back to an earlier state.
            torch.save({"model": model.state_dict(), "step": step, "config": asdict(cfg),
                        "chars": corpus.vocab["chars"], "eos": args.eos}, args.run_dir / f"step{step}.pt")
        if step % args.eval_every == 0 or step == args.steps:
            msg = " ".join(f"{k}={v:.2f}" for k, v in validate(model, corpus, device, args.eos).items())
            print(f"perplexity at step {step}: {msg}", flush=True)
            log.write(f"{step}\tval\t{msg}\n")
            log.flush()
    save()


if __name__ == "__main__":
    main()
