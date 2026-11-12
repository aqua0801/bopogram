"""Aggregate cleaning statistics and draw a random sentence sample for review.

Usage:
    uv run python pipeline/report.py CLEAN_ROOT REPORT_DIR [--sample 200] [--seed 0]

Input : CLEAN_ROOT/<source>/part-*.stats.json and part-*.jsonl.gz (from clean.py)
Output: REPORT_DIR/corpus_stats.md   per-source statistics (Traditional Chinese)
        REPORT_DIR/sample.txt        random sentences, "<source>\t<sentence>"
        REPORT_DIR/frequent_sentences.txt  most repeated sentences (boilerplate candidates)

Streaming: shards are read line by line. The sample uses reservoir sampling,
so it is uniform over all sentences without holding the corpus in memory.
Repeated-sentence counting only tracks short sentences (<= 12 chars) to keep
the counter small.
"""

from __future__ import annotations

import argparse
import gzip
import json
import random
from collections import Counter
from pathlib import Path

HIST_ORDER = ["0", "<0.5", "<1", "<2", "<5", ">=5"]
SHORT = 12


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("clean_root", type=Path)
    ap.add_argument("report_dir", type=Path)
    ap.add_argument("--sample", type=int, default=200)
    ap.add_argument("--seed", type=int, default=0)
    args = ap.parse_args()
    args.report_dir.mkdir(parents=True, exist_ok=True)
    rng = random.Random(args.seed)

    reservoir: list[tuple[str, str]] = []
    seen_sents = 0
    lines = ["# 語料統計報告", ""]
    table = ["| 來源 | 輸入文章 | 保留文章 | 重複 | 句數 | 中文字數 | 約略大小 |",
             "|---|---:|---:|---:|---:|---:|---:|"]
    details: list[str] = []
    grand = Counter()
    freq: Counter = Counter()

    for src_dir in sorted(p for p in args.clean_root.iterdir() if p.is_dir()):
        stats = Counter()
        hist = Counter()
        terms = Counter()
        for sf in sorted(src_dir.glob("part-*.stats.json")):
            s = json.loads(sf.read_text())
            hist.update(s.pop("density_hist", {}))
            terms.update(s.pop("term_hits", {}))
            stats.update(s)
        if not stats:
            continue
        for shard in sorted(src_dir.glob("part-*.jsonl.gz")):
            with gzip.open(shard, "rt", encoding="utf-8") as fh:
                for line in fh:
                    for sent in json.loads(line)["sents"]:
                        seen_sents += 1
                        if len(sent) <= SHORT:
                            freq[sent] += 1
                        if len(reservoir) < args.sample:
                            reservoir.append((src_dir.name, sent))
                        else:
                            j = rng.randrange(seen_sents)
                            if j < args.sample:
                                reservoir[j] = (src_dir.name, sent)
        grand.update(stats)
        table.append(f"| {src_dir.name} | {stats['docs_in']:,} | {stats['docs_out']:,} | "
                     f"{stats['docs_duplicate']:,} | {stats['sentences']:,} | "
                     f"{stats['cjk_chars']:,} | {stats['cjk_chars'] * 3 / 1e9:.2f} GB |")
        total_docs = max(1, sum(hist.values()))
        details += [f"## {src_dir.name}", "",
                    "中國用語密度（強級用語次數／每千字）的文章分布：", "",
                    "| 密度 | 文章數 | 比例 |", "|---|---:|---:|"]
        details += [f"| {b} | {hist.get(b, 0):,} | {hist.get(b, 0) / total_docs:.1%} |" for b in HIST_ORDER]
        top = "、".join(f"{t}（{n:,}）" for t, n in terms.most_common(15))
        details += ["", f"最常出現的對照用語：{top or '無'}", ""]

    table.append(f"| **合計** | {grand['docs_in']:,} | {grand['docs_out']:,} | "
                 f"{grand['docs_duplicate']:,} | {grand['sentences']:,} | "
                 f"{grand['cjk_chars']:,} | {grand['cjk_chars'] * 3 / 1e9:.2f} GB |")
    lines += table + ["", "大小以每個中文字 3 bytes（UTF-8）估算。", ""] + details
    (args.report_dir / "corpus_stats.md").write_text("\n".join(lines), encoding="utf-8")

    rng.shuffle(reservoir)
    (args.report_dir / "sample.txt").write_text(
        "".join(f"{src}\t{sent}\n" for src, sent in reservoir), encoding="utf-8")
    (args.report_dir / "frequent_sentences.txt").write_text(
        "".join(f"{n}\t{s}\n" for s, n in freq.most_common(300)), encoding="utf-8")
    print(f"{seen_sents:,} sentences scanned; report written to {args.report_dir}")


if __name__ == "__main__":
    main()
