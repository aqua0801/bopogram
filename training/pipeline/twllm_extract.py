"""Extract real user prompts from the TWLLM conversation dataset.

Source: yentinglin/twllm-data (Apache-2.0)
        https://huggingface.co/datasets/yentinglin/twllm-data

Usage:
    uv run python pipeline/twllm_extract.py TRAIN.parquet OUT_DIR

Only `human` turns are kept. Assistant turns are model-generated and are
dropped, as are whole subsets whose prompts are synthetic (dataset name
containing "evol" or "synthetic"). The parquet file is read in batches.
Output: OUT_DIR/part-twllm.jsonl.gz   {"id","source","title","text"}
"""

from __future__ import annotations

import argparse
import gzip
import json
import os
from collections import Counter
from pathlib import Path

import pyarrow.parquet as pq

SYNTHETIC_MARKERS = ("evol", "synthetic")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("parquet", type=Path)
    ap.add_argument("out_dir", type=Path)
    args = ap.parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)

    kept: Counter = Counter()
    skipped: Counter = Counter()
    tmp = args.out_dir / "part-twllm.tmp"
    row = 0
    with gzip.open(tmp, "wt", encoding="utf-8") as fh:
        for batch in pq.ParquetFile(args.parquet).iter_batches(batch_size=1000):
            for rec in batch.to_pylist():
                row += 1
                subset = rec.get("dataset") or ""
                if any(m in subset.lower() for m in SYNTHETIC_MARKERS):
                    skipped[subset] += 1
                    continue
                turns = [t["content"] for t in rec.get("conversations") or []
                         if t.get("role") == "human" and t.get("content")]
                text = "\n".join(turns)
                if len(text) < 4:
                    continue
                kept[subset] += 1
                fh.write(json.dumps({"id": f"twllm-{row}", "source": "twllm",
                                     "title": subset, "text": text}, ensure_ascii=False) + "\n")
    os.replace(tmp, args.out_dir / "part-twllm.jsonl.gz")
    print("kept:", dict(kept))
    print("skipped (synthetic prompts):", dict(skipped))


if __name__ == "__main__":
    main()
