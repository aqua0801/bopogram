"""Keep the conversational part of extracted FineWeb2 pages: blogs and forums.

Usage:
    uv run python pipeline/fineweb_conv.py EXTRACTED_DIR OUT_DIR

Input : EXTRACTED_DIR/part-fineweb-*.jsonl.gz   (fineweb_extract.py)
Output: OUT_DIR/part-fineweb-*.jsonl.gz         same records, only pages whose host
                                                ends with one of CONVERSATIONAL

Personal blogs and forums are written the way people talk and type, unlike the
news, government and encyclopedia text of the other sources; they are what the
re-ranking data for everyday typing is drawn from (prototype/docs/charlm-rerank-training.md).
Sites that forbid AI training were already removed by fineweb_extract.py.
Streaming: one shard at a time, line by line; finished shards are skipped.
"""

from __future__ import annotations

import argparse
import gzip
import json
import os
from pathlib import Path
from urllib.parse import urlsplit

CONVERSATIONAL = (
    "pixnet.net", "blogspot.com", "xuite.net", "yam.com", "wordpress.com", "fc2.com",
    "gamer.com.tw", "babyhome.com.tw", "vocus.cc", "medium.com", "udn.com/blog",
)


def keep(url: str) -> bool:
    parts = urlsplit(url)
    host = (parts.hostname or "").lower()
    for d in CONVERSATIONAL:
        if "/" in d:
            h, path = d.split("/", 1)
            if (host == h or host.endswith("." + h)) and parts.path.startswith("/" + path):
                return True
        elif host == d or host.endswith("." + d):
            return True
    return False


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("extracted_dir", type=Path)
    ap.add_argument("out_dir", type=Path)
    args = ap.parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)
    for src in sorted(args.extracted_dir.glob("part-fineweb-*.jsonl.gz")):
        dest = args.out_dir / src.name
        if dest.exists():
            continue
        tmp = dest.with_suffix(".tmp")
        kept = total = 0
        with gzip.open(src, "rt", encoding="utf-8") as fin, gzip.open(tmp, "wt", encoding="utf-8") as fout:
            for line in fin:
                total += 1
                if keep(json.loads(line).get("url", "")):
                    fout.write(line)
                    kept += 1
        os.replace(tmp, dest)
        print(f"{src.name}: {kept} of {total} pages", flush=True)


if __name__ == "__main__":
    main()
