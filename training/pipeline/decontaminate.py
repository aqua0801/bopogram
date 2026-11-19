"""Drop corpus sentences that overlap the evaluation sets (web text may quote them).

Usage:
    uv run python pipeline/decontaminate.py CORPUS_SRC_DIR OUT_DIR EVAL_TSV [EVAL_TSV ...] [--workers 8]

Input : CORPUS_SRC_DIR/part-*.txt.gz   one sentence per line (from mix.py)
        EVAL_TSV                       eval/testset/testset.tsv, eval/devset/moztw_dev.tsv
                                       (columns id, category, context, text, note)
Output: OUT_DIR/part-*.txt.gz, OUT_DIR/decontaminate.json

A sentence is dropped when it shares any run of WINDOW characters (Chinese
characters only, punctuation ignored) with an evaluation context or text, or,
for evaluation texts shorter than WINDOW, when it is that text (a short phrase
inside a longer sentence is ordinary language, not a leak).
Shards are written under .tmp and renamed; finished shards are skipped.
"""

from __future__ import annotations

import argparse
import csv
import gzip
import json
import os
import re
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

WINDOW = 8
_NON_CJK = re.compile(r"[^一-鿿㐀-䶿]+")

_grams: set[str] = set()
_short: set[str] = set()


def windows(s: str):
    for part in _NON_CJK.split(s):
        for i in range(len(part) - WINDOW + 1):
            yield part[i: i + WINDOW]


def load_eval(paths: list[Path]) -> tuple[set[str], set[str]]:
    grams, short = set(), set()
    for p in paths:
        with open(p, encoding="utf-8", newline="") as fh:
            for row in csv.DictReader(fh, delimiter="\t"):
                for field in ("context", "text"):
                    s = row.get(field) or ""
                    cjk = _NON_CJK.sub("", s)
                    found = set(windows(s))
                    grams |= found
                    if not found and field == "text" and len(cjk) >= 4:
                        short.add(cjk)
    return grams, short


def init(grams: set[str], short: set[str]) -> None:
    global _grams, _short
    _grams, _short = grams, short


def overlaps(line: str) -> bool:
    if any(g in _grams for g in windows(line)):
        return True
    return _NON_CJK.sub("", line) in _short


def run_shard(src: Path, dst: Path) -> tuple[str, int, int]:
    kept = dropped = 0
    tmp = dst.with_suffix(".tmp")
    with gzip.open(src, "rt", encoding="utf-8") as fin, gzip.open(tmp, "wt", encoding="utf-8") as fout:
        for line in fin:
            if overlaps(line):
                dropped += 1
            else:
                kept += 1
                fout.write(line)
    os.replace(tmp, dst)
    return src.name, kept, dropped


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("src", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("eval", type=Path, nargs="+")
    ap.add_argument("--workers", type=int, default=8)
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    grams, short = load_eval(args.eval)
    print(f"{len(grams)} evaluation windows of {WINDOW} characters, {len(short)} short texts", flush=True)
    todo = [s for s in sorted(args.src.glob("part-*.txt.gz")) if not (args.out / s.name).exists()]
    total = {"kept": 0, "dropped": 0}
    with ProcessPoolExecutor(args.workers, initializer=init, initargs=(grams, short)) as pool:
        for name, k, d in pool.map(run_shard, todo, [args.out / s.name for s in todo]):
            total["kept"] += k
            total["dropped"] += d
            print(f"{name}: kept {k}, dropped {d}", flush=True)
    (args.out / "decontaminate.json").write_text(json.dumps({**total, "window": WINDOW,
                                                             "eval": [str(p) for p in args.eval]}))


if __name__ == "__main__":
    main()
