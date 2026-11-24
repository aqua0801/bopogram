"""Write an ARPA file with the values the engine actually uses: those of the
quantized KenLM binary, not of the ARPA it was built from.

Usage (WSL):
    uv run --project ../../prototype python export_arpa.py SOURCE.arpa MODEL.binary OUT.arpa

The binaries are built with `build_binary -q 8 -b 8 trie` (train.py), so the
probabilities of the 2- and 3-grams and the back-off weights of the 2-grams are
rounded to 256 values per table. Another engine that loads the source ARPA
would score differently from the Python one. This script streams the source
ARPA and replaces every value by what the binary returns:

  * unigrams are not quantized: copied as they are;
  * an n-gram's probability (n >= 2) is the binary's score of its last word
    after the others; since the n-gram exists, no back-off is involved;
  * a back-off weight B(c) is solved exactly as a float32: KenLM's scores of
    words after c and after c without its first word give equations
    fl32(lower + B) = score (see Prober and backoff_table). Contexts are grouped
    by an estimate of B, so every group shares one stored value.
    A few groups keep several float32 candidates that satisfy every equation
    seen; the one nearest to the estimate is used, which can be 1 float32 step
    off for a few words (see prototype/docs/lm-arpa.md for the counts).

In the output, every order >= 2 has at most 256 distinct probabilities and the
2-grams at most 256 distinct back-off weights, so an engine can store them as
8-bit indices without losing anything.

Checkpoints: the two passes that solve the back-off weights save their progress
to OUT.arpa.o2.npz about every minute, and writing the output saves it every
CHUNK lines (OUT.arpa.part, OUT.arpa.ckpt). Run the same command again to
continue. Finished when OUT.arpa exists. The largest runtime model takes about
25 minutes.
"""

from __future__ import annotations

import argparse
import bisect
import json
import os
import sys
import time
from collections import Counter
from pathlib import Path

import kenlm
import numpy as np

CHUNK = 2_000_000
GAP = 1e-6
FOLLOWERS = 200     # probe words per history, the most likely ones
FOLLOWER_MIN = -3.0 # ... with at least this log10 probability
MAX_CONTEXTS = 3000 # contexts tried per back-off value
COMMON = 1000       # ... and the most common words as probes too
SEARCH_ULPS = 4096  # float32 steps searched around the estimate
ESTIMATE_PROBES = 50  # probe words tried for an estimate
SAVE_SECONDS = 60   # how often the first two passes save their progress
SAFE_GAP = 1e-5     # groups this far apart can be told apart by the <unk> estimate


def states(model: kenlm.Model, words: list[str]) -> kenlm.State:
    """The model state after `words` (a leading <s> starts a sentence)."""
    s = kenlm.State()
    if words and words[0] == "<s>":
        model.BeginSentenceWrite(s)
        words = words[1:]
    else:
        model.NullContextWrite(s)
    for w in words:
        out = kenlm.State()
        model.BaseScore(s, w, out)
        s = out
    return s


def score(model: kenlm.Model, context: list[str], word: str) -> float:
    return model.BaseScore(states(model, context), word, kenlm.State())


def raw_backoff(model: kenlm.Model, words: list[str]) -> float:
    return score(model, words, "<unk>") - score(model, words[1:], "<unk>")


def section_lines(source: Path, order: int):
    """The fields of the n-gram lines of one order."""
    with open(source, encoding="utf-8") as fh:
        section = 0
        for line in fh:
            if line.startswith("\\") and line.rstrip().endswith("-grams:"):
                section = int(line[1:line.index("-")])
                continue
            if section > order:
                return
            if section == order:
                parts = line.rstrip("\n").split("\t")
                if len(parts) >= 2:
                    yield parts


class Prober:
    """Equations for back-off weights. For a context c = (a, b) and a word w where
    scoring w after c and after b ends on the same n-gram, KenLM returns
    fl32(score(w | b) + B(c)): it adds the back-off weights from the shortest
    history up, so B(c) comes last. The closer score(w | b) is to 0, the finer the
    float32 precision of the sum, so the words most likely after b are tried first,
    then the most common words."""

    def __init__(self, model: kenlm.Model, source: Path, order: int):
        self.model = model
        followers: dict[str, list[tuple[float, str]]] = {}
        for parts in section_lines(source, order):
            p = float(parts[0])
            if p >= FOLLOWER_MIN:
                *hist, w = parts[1].split(" ")
                followers.setdefault(" ".join(hist), []).append((p, w))
        self.followers = {h: [w for _, w in sorted(lst, reverse=True)[:FOLLOWERS]] for h, lst in followers.items()}
        self.common = [w for _, w in sorted(((float(p[0]), p[1]) for p in section_lines(source, 1)), reverse=True)
                       if w not in ("<s>", "<unk>")][:COMMON]

    def equations(self, words: list[str], limit: int | None = None, probes: int | None = None):
        """(lower score, KenLM's score) pairs: KenLM's score = fl32(lower + B).
        At most `limit` pairs, from at most `probes` probe words."""
        m = self.model
        sc, sb = states(m, words), states(m, words[1:])
        out = []
        for w in (self.followers.get(" ".join(words[1:]), []) + self.common)[:probes]:
            full = m.BaseFullScore(sc, w, kenlm.State())
            low = m.BaseFullScore(sb, w, kenlm.State())
            if full.ngram_length == low.ngram_length <= len(words) and not full.oov:
                out.append((np.float32(low.log_prob), np.float32(full.log_prob)))
                if limit and len(out) >= limit:
                    break
        return out

    def estimate(self, words: list[str]) -> float:
        """B(words) to within a few float32 steps of the smaller sum's magnitude."""
        eqs = self.equations(words, limit=3, probes=ESTIMATE_PROBES)
        if not eqs:
            return raw_backoff(self.model, words)
        low, full = max(eqs, key=lambda e: e[0])  # the sum nearest to 0 is the finest
        return float(full) - float(low)


def backoff_table(prober: Prober, source: Path, order: int, work: Path) -> tuple[list[float], list[float], int]:
    """(group estimates, exact value of each group, groups not pinned down to one
    float32 value). The estimates are grouped, split where sorted values are more
    than GAP apart; each group's value is the float32 B that satisfies every
    equation (Prober) of up to MAX_CONTEXTS of its contexts. When several remain,
    the one nearest to the estimate is used (they are a few float32 steps apart).
    Both passes save their progress to `work` (.npz) about every SAVE_SECONDS."""
    saved = dict(np.load(work)) if work.exists() else {}
    last = time.monotonic()

    def due() -> bool:
        nonlocal last
        if time.monotonic() - last < SAVE_SECONDS:
            return False
        last = time.monotonic()
        return True

    def save(**arrays) -> None:
        tmp = work.with_name(work.name + ".tmp.npz")
        np.savez(tmp, **arrays)
        tmp.replace(work)

    # Pass 1: an estimate for every context with a back-off weight.
    est = list(saved.get("est", []))
    if not saved.get("est_done"):
        n = 0
        for parts in section_lines(source, order):
            if len(parts) != 3:
                continue
            n += 1
            if n <= len(est):
                continue
            est.append(prober.estimate(parts[1].split(" ")))
            if due():
                save(est=np.array(est))
        save(est=np.array(est), est_done=np.array(1))
    values = sorted(est)
    groups, group = [], []
    for v in values + [float("inf")]:
        if group and v - group[-1] > GAP:
            groups.append(Counter(group).most_common(1)[0][0])
            group = []
        group.append(v)
    # Pass 2: solve each group's value from its contexts.
    steps = np.arange(-SEARCH_ULPS, SEARCH_ULPS + 1)
    cands = [(np.float32(g).view(np.int32).astype(np.int64) + steps).astype(np.int32).view(np.float32)
             for g in groups]
    if "masks" in saved:
        masks = list(np.unpackbits(saved["masks"], axis=1)[:, :len(steps)].astype(bool))
        tried, done_lines = list(saved["tried"]), int(saved["lines"])
    else:
        masks, tried, done_lines = [np.ones(len(c), dtype=bool) for c in cands], [0] * len(groups), 0

    def save_masks(lines: int) -> None:
        save(est=np.array(est), est_done=np.array(1), masks=np.packbits(np.array(masks), axis=1),
             tried=np.array(tried), lines=np.array(lines))

    closed = lambda k: tried[k] >= MAX_CONTEXTS or masks[k].sum() == 1
    open_groups = sum(not closed(k) for k in range(len(groups)))
    n = 0
    for parts in section_lines(source, order):
        if not open_groups:
            break
        if len(parts) != 3:
            continue
        n += 1
        if n <= done_lines:
            continue
        k = snap_index(groups, est[n - 1])
        if closed(k):
            continue
        tried[k] += 1
        for low, full in prober.equations(parts[1].split(" ")):
            m = masks[k] & ((low + cands[k]) == full)
            if m.any():  # an equation that rules out everything is ignored, not trusted
                masks[k] = m
        open_groups -= closed(k)
        if due():
            save_masks(n)
    exact, unsure = [], 0
    for g, c, m in zip(groups, cands, masks):
        ok = c[m]
        exact.append(float(ok[np.argmin(np.abs(ok.astype(np.float64) - g))]))
        unsure += len(ok) != 1
    return groups, exact, unsure


def lookup(table: list[list[float]], prober: Prober, words: list[str]) -> float:
    """The exact back-off weight of `words`: the value of its group. The cheap
    estimate decides unless a neighbouring group is too close to tell apart."""
    groups, exact = table
    v = raw_backoff(prober.model, words)
    k = snap_index(groups, v)
    if all(abs(groups[j] - groups[k]) > SAFE_GAP for j in (k - 1, k + 1) if 0 <= j < len(groups)):
        return exact[k]
    return exact[snap_index(groups, prober.estimate(words))]


def snap_index(table: list[float], v: float) -> int:
    i = bisect.bisect_left(table, v)
    near = [k for k in (i - 1, i) if 0 <= k < len(table)]
    return min(near, key=lambda k: abs(table[k] - v))


def fmt(v: float) -> str:
    """The shortest text that reads back as exactly this float32."""
    return str(np.float32(v))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("source", type=Path)
    ap.add_argument("binary", type=Path)
    ap.add_argument("out", type=Path)
    args = ap.parse_args()
    if args.out.exists():
        print(f"{args.out} exists: nothing to do")
        return
    model = kenlm.Model(str(args.binary))
    max_order = model.order
    part, ckpt = Path(str(args.out) + ".part"), Path(str(args.out) + ".ckpt")
    state = json.loads(ckpt.read_text()) if ckpt.exists() else None
    probers = {o: Prober(model, args.source, o) for o in range(2, max_order)}
    if state is None:
        tables = {}
        for o, prober in probers.items():
            work = Path(f"{args.out}.o{o}.npz")
            groups, exact, unsure = backoff_table(prober, args.source, o, work)
            tables[str(o)] = [groups, exact]
            print(f"order {o} back-off weights: {len(set(exact))} values ({len(groups)} groups), "
                  f"{unsure} groups not pinned down to one float32 value", flush=True)
        state = {"lines": 0, "bytes": 0, "tables": tables}
        ckpt.write_text(json.dumps(state))
        for o in probers:
            Path(f"{args.out}.o{o}.npz").unlink(missing_ok=True)
    tables = {int(o): t for o, t in state["tables"].items()}
    mode = "r+b" if part.exists() and state["lines"] else "wb"  # resume the last pass
    with open(args.source, encoding="utf-8") as src, open(part, mode) as out:
        out.seek(state["bytes"])
        out.truncate()
        section = 0
        for n, line in enumerate(src):
            if line.startswith("\\") and line.rstrip().endswith("-grams:"):
                section = int(line[1:line.index("-")])
            if n < state["lines"]:
                continue
            parts = line.rstrip("\n").split("\t")
            if section >= 2 and len(parts) >= 2:
                words = parts[1].split(" ")
                parts[0] = fmt(score(model, words[:-1], words[-1]))
                if len(parts) == 3:
                    parts[2] = fmt(lookup(tables[section], probers[section], words))
                line = "\t".join(parts) + "\n"
            out.write(line.encode("utf-8"))
            if (n + 1) % CHUNK == 0:
                out.flush()
                os.fsync(out.fileno())
                state.update(lines=n + 1, bytes=out.tell())
                ckpt.write_text(json.dumps(state))
                print(f"{n + 1:,} lines", flush=True)
    part.replace(args.out)
    ckpt.unlink(missing_ok=True)
    print(f"done: {args.out}")


if __name__ == "__main__":
    sys.exit(main())
