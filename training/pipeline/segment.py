"""Segment the mixed corpus into dictionary words (unigram Viterbi).

Usage:
    # 1. estimate word counts from a sample (every Nth shard)
    uv run python pipeline/segment.py count CORPUS_DIR DICT_TSV COUNTS_TSV \
        [--prior twngrams/token_1gram.tsv] [--init-counts COUNTS] [--every 10]
    # 2. segment everything with those counts
    uv run python pipeline/segment.py run CORPUS_DIR DICT_TSV COUNTS_TSV OUT_DIR

Input : CORPUS_DIR/<source>/part-*.txt.gz   one sentence per line (from mix.py)
Output: OUT_DIR/<source>/part-*.txt.gz      same lines, tokens separated by spaces

Every token of a Chinese run is a dictionary word, so the language model's
vocabulary always has a pronunciation. Characters missing from the
dictionary become single-character tokens. Latin/digit runs and punctuation
are kept as their own tokens.

Streaming / resume: shards are processed independently (one worker process
per shard, line by line); an output shard is written to a .tmp name and
renamed, and existing output shards are skipped.
"""

from __future__ import annotations

import argparse
import gzip
import math
import os
import re
import sys
from collections import Counter
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

MAX_LEN = 8
_RE_PIECE = re.compile(r"([一-鿿㐀-䶿]+)|([A-Za-z0-9][A-Za-z0-9.%+\-]*)|(\S)")

_cost: dict[str, float] = {}
_unk_cost = 0.0
_default_cost = 0.0


def load_words(dict_tsv: Path) -> set[str]:
    words = set()
    with open(dict_tsv, encoding="utf-8") as fh:
        for line in fh:
            words.add(line.split("\t", 1)[0])
    return words


def init_costs(dict_tsv: str, counts_tsv: str | None, prior_tsv: str | None) -> None:
    """Word cost = -log P(word); words never seen get a small pseudo-count."""
    global _cost, _unk_cost, _default_cost
    words = load_words(Path(dict_tsv))
    counts: Counter = Counter()
    for path in (counts_tsv, prior_tsv):
        if path and os.path.exists(path):
            with open(path, encoding="utf-8") as fh:
                for line in fh:
                    a, _, b = line.rstrip("\n").partition("\t")
                    # counts file: word<TAB>count ; twngrams prior: count<TAB>word
                    word, n = (b, a) if a.isdigit() else (a, b)
                    if word in words and n.isdigit():
                        counts[word] += int(n)
            if counts:
                break  # corpus counts take precedence over the prior
    total = sum(counts.values()) + 0.5 * len(words)
    _cost = {w: -math.log((counts.get(w, 0) + 0.5) / total) for w in words}
    _default_cost = -math.log(0.5 / total)
    _unk_cost = _default_cost + 5.0


def segment_run(text: str) -> list[str]:
    n = len(text)
    best = [0.0] + [math.inf] * n
    back = [0] * (n + 1)
    cost = _cost
    for i in range(n):
        base = best[i]
        if base == math.inf:
            continue
        # single character is always allowed
        c = base + cost.get(text[i], _unk_cost)
        if c < best[i + 1]:
            best[i + 1] = c
            back[i + 1] = i
        for j in range(i + 2, min(n, i + MAX_LEN) + 1):
            w = cost.get(text[i:j])
            if w is not None and base + w < best[j]:
                best[j] = base + w
                back[j] = i
    out = []
    j = n
    while j > 0:
        i = back[j]
        out.append(text[i:j])
        j = i
    out.reverse()
    return out


def segment_line(line: str) -> list[str]:
    tokens: list[str] = []
    for m in _RE_PIECE.finditer(line):
        if m.group(1):
            tokens.extend(segment_run(m.group(1)))
        else:
            tokens.append(m.group(0))
    return tokens


def count_shard(path: str) -> Counter:
    counts: Counter = Counter()
    with gzip.open(path, "rt", encoding="utf-8") as fh:
        for line in fh:
            for m in _RE_PIECE.finditer(line):
                if m.group(1):
                    counts.update(segment_run(m.group(1)))
    return counts


def run_shard(job: tuple[str, str]) -> tuple[str, int, int]:
    src, dst = job
    tmp = dst + ".tmp"
    lines = toks = 0
    with gzip.open(src, "rt", encoding="utf-8") as fin, \
            gzip.open(tmp, "wt", encoding="utf-8", compresslevel=5) as fout:
        for line in fin:
            t = segment_line(line.rstrip("\n"))
            if t:
                fout.write(" ".join(t) + "\n")
                lines += 1
                toks += len(t)
    os.replace(tmp, dst)
    return dst, lines, toks


def shards(corpus_dir: Path) -> list[Path]:
    return sorted(corpus_dir.glob("*/part-*.txt.gz"))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("count")
    c.add_argument("corpus_dir", type=Path)
    c.add_argument("dict_tsv", type=Path)
    c.add_argument("counts_tsv", type=Path)
    c.add_argument("--prior", type=Path)
    c.add_argument("--every", type=int, default=10)
    c.add_argument("--init-counts", type=Path,
                   help="counts from a previous `count` pass; re-estimating once sharpens them")
    r = sub.add_parser("run")
    r.add_argument("corpus_dir", type=Path)
    r.add_argument("dict_tsv", type=Path)
    r.add_argument("counts_tsv", type=Path)
    r.add_argument("out_dir", type=Path)
    for p in (c, r):
        p.add_argument("--workers", type=int, default=max(1, (os.cpu_count() or 2) - 4))
    args = ap.parse_args()

    if args.cmd == "count":
        by_source: dict[str, list[Path]] = {}
        for s in shards(args.corpus_dir):
            by_source.setdefault(s.parent.name, []).append(s)
        sample = [str(p) for group in by_source.values() for p in group[:: args.every]]
        print(f"counting on {len(sample)} sample shards", flush=True)
        total: Counter = Counter()
        with ProcessPoolExecutor(args.workers, initializer=init_costs,
                                 initargs=(str(args.dict_tsv),
                                           str(args.init_counts) if args.init_counts else None,
                                           str(args.prior) if args.prior else None)) as pool:
            for counts in pool.map(count_shard, sample):
                total.update(counts)
        tmp = args.counts_tsv.with_suffix(".tmp")
        with open(tmp, "w", encoding="utf-8") as fh:
            for word, n in total.most_common():
                fh.write(f"{word}\t{n}\n")
        os.replace(tmp, args.counts_tsv)
        print(f"{len(total):,} distinct tokens, {sum(total.values()):,} tokens")
        return

    jobs = []
    for s in shards(args.corpus_dir):
        dst = args.out_dir / s.parent.name / s.name
        dst.parent.mkdir(parents=True, exist_ok=True)
        if not dst.exists():
            jobs.append((str(s), str(dst)))
    print(f"{len(jobs)} shards to segment", flush=True)
    lines = toks = 0
    with ProcessPoolExecutor(args.workers, initializer=init_costs,
                             initargs=(str(args.dict_tsv), str(args.counts_tsv), None)) as pool:
        for k, (dst, n_lines, n_toks) in enumerate(pool.map(run_shard, jobs), 1):
            lines += n_lines
            toks += n_toks
            if k % 10 == 0 or k == len(jobs):
                print(f"{k}/{len(jobs)} shards, {lines:,} lines, {toks:,} tokens", flush=True)
    print("finished", file=sys.stderr)


if __name__ == "__main__":
    main()
