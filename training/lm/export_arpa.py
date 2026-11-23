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
  * a back-off weight b(c) is score(<unk> | c) - score(<unk> | c without its
    first word): every back-off weight below c cancels out. The difference can
    be off by float rounding (about 1e-6), so each value is snapped to the
    nearest of the table's values, found in a first pass.

In the output, every order >= 2 has at most 256 distinct probabilities and the
2-grams at most 256 distinct back-off weights, so an engine can store them as
8-bit indices without losing anything.

Checkpoints: progress is saved every CHUNK lines; run the same command again to
continue. Finished when OUT.arpa exists (the work file is OUT.arpa.part).
"""

from __future__ import annotations

import argparse
import bisect
import json
import os
import sys
from collections import Counter
from pathlib import Path

import kenlm

CHUNK = 2_000_000
GAP = 1e-5


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


def backoff_table(model: kenlm.Model, source: Path, order: int) -> list[float]:
    """The distinct back-off weights of one order. The differences scatter by about
    1e-6 around each stored value; groups are split where sorted values are more
    than GAP apart, and the most common exact value of a group is the stored one."""
    values: list[float] = []
    with open(source, encoding="utf-8") as fh:
        section = 0
        for line in fh:
            if line.startswith("\\") and line.rstrip().endswith("-grams:"):
                section = int(line[1:line.index("-")])
                continue
            if section > order:
                break
            if section != order:
                continue
            parts = line.rstrip("\n").split("\t")
            if len(parts) == 3:
                values.append(raw_backoff(model, parts[1].split(" ")))
    values.sort()
    table, group = [], []
    for v in values + [float("inf")]:
        if group and v - group[-1] > GAP:
            table.append(Counter(group).most_common(1)[0][0])
            group = []
        group.append(v)
    return table


def snap(table: list[float], v: float) -> float:
    i = bisect.bisect_left(table, v)
    near = [table[k] for k in (i - 1, i) if 0 <= k < len(table)]
    return min(near, key=lambda t: abs(t - v)) if near else v


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
    state = json.loads(ckpt.read_text()) if ckpt.exists() and part.exists() else None
    if state is None:
        tables = {str(o): backoff_table(model, args.source, o) for o in range(2, max_order)}
        state = {"lines": 0, "bytes": 0, "tables": tables}
        print("back-off tables: " + ", ".join(f"order {o}: {len(t)} values" for o, t in tables.items()), flush=True)
    tables = {int(o): t for o, t in state["tables"].items()}
    mode = "r+b" if part.exists() and state["lines"] else "wb"
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
                parts[0] = f"{score(model, words[:-1], words[-1]):.7g}"
                if len(parts) == 3:
                    parts[2] = f"{snap(tables[section], raw_backoff(model, words)):.7g}"
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
