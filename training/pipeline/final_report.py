"""Write the phase-1 deliverables for the mixed corpus.

Usage:
    uv run python pipeline/final_report.py CORPUS_DIR REPORT_DIR [--sample 200] [--seed 0]

Input : CORPUS_DIR from mix.py (plan.json, totals.json, <source>/part-*.txt.gz)
Output: REPORT_DIR/final_corpus.md    composition and filtering summary (Traditional Chinese)
        REPORT_DIR/final_sample.txt   uniform random sample of sentences, "<source>\t<sentence>"

The sample is drawn by reservoir sampling over a single streaming pass.
"""

from __future__ import annotations

import argparse
import gzip
import json
import random
from pathlib import Path


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("corpus_dir", type=Path)
    ap.add_argument("report_dir", type=Path)
    ap.add_argument("--sample", type=int, default=200)
    ap.add_argument("--seed", type=int, default=0)
    args = ap.parse_args()
    args.report_dir.mkdir(parents=True, exist_ok=True)

    plan = json.loads((args.corpus_dir / "plan.json").read_text())
    totals = json.loads((args.corpus_dir / "totals.json").read_text())

    rng = random.Random(args.seed)
    reservoir: list[tuple[str, str]] = []
    seen = 0
    for src in sorted(totals):
        for shard in sorted((args.corpus_dir / src).glob("part-*.txt.gz")):
            with gzip.open(shard, "rt", encoding="utf-8") as fh:
                for line in fh:
                    seen += 1
                    if len(reservoir) < args.sample:
                        reservoir.append((src, line.rstrip("\n")))
                    else:
                        j = rng.randrange(seen)
                        if j < args.sample:
                            reservoir[j] = (src, line.rstrip("\n"))
    rng.shuffle(reservoir)
    (args.report_dir / "final_sample.txt").write_text(
        "".join(f"{s}\t{t}\n" for s, t in reservoir), encoding="utf-8")

    grand_chars = sum(t["cjk_chars"] for t in totals.values())
    grand_sents = sum(t["sentences"] for t in totals.values())
    lines = [
        "# 第 1 階段：最終語料組成", "",
        f"- 中國用語密度門檻：每千字 {plan['max_cn_density']} 次以上的文章整篇丟棄",
        f"- 佔比上限：{plan['caps'] or '無'}",
        f"- 合計：{grand_sents:,} 句、{grand_chars:,} 個中文字（約 {grand_chars * 3 / 1e9:.2f} GB）", "",
        "| 來源 | 清理後文章 | 因中國用語丟棄 | 因模板殘句丟棄 | 抽樣比例 | 最終文章 | 最終句數 | 最終中文字數 | 佔比 |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for src in sorted(totals, key=lambda s: -totals[s]["cjk_chars"]):
        st, t = plan["stats"][src], totals[src]
        lines.append(
            f"| {src} | {st.get('docs', 0):,} | {st.get('dropped_cn', 0):,} | "
            f"{st.get('dropped_residue', 0):,} | {plan['rates'][src]:.1%} | {t['docs']:,} | "
            f"{t['sentences']:,} | {t['cjk_chars']:,} | {t['share']:.1%} |")
    lines += ["", f"抽樣檔：`final_sample.txt`（{len(reservoir)} 句，對全部 {seen:,} 句均勻抽樣）", ""]
    (args.report_dir / "final_corpus.md").write_text("\n".join(lines), encoding="utf-8")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
