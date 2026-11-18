"""Rank the sites in extracted FineWeb2 pages by how much text they contribute.

Usage:
    uv run python pipeline/fineweb_domains.py EXTRACTED_DIR [--top 50]

Hosts are grouped by registered domain (blog.example.com.tw -> example.com.tw),
so the list names the sites whose terms need checking.
"""

from __future__ import annotations

import argparse
import gzip
import json
from collections import Counter
from pathlib import Path
from urllib.parse import urlsplit

# Second-level labels under which the registered domain has three labels.
SECOND_LEVEL = {"com", "org", "net", "edu", "gov", "idv", "game", "club", "ebiz", "co", "ac", "or", "ne", "go"}


def site(url: str) -> str:
    labels = (urlsplit(url).hostname or "").lower().split(".")
    n = 3 if len(labels) >= 3 and labels[-2] in SECOND_LEVEL and len(labels[-1]) == 2 else 2
    return ".".join(labels[-n:])


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("extracted_dir", type=Path)
    ap.add_argument("--top", type=int, default=50)
    args = ap.parse_args()
    chars: Counter = Counter()
    pages: Counter = Counter()
    for path in sorted(args.extracted_dir.glob("part-fineweb-*.jsonl.gz")):
        with gzip.open(path, "rt", encoding="utf-8") as fh:
            for line in fh:
                doc = json.loads(line)
                s = site(doc["url"])
                chars[s] += len(doc["text"])
                pages[s] += 1
    total = sum(chars.values())
    print(f"{len(chars)} sites, {total / 1e6:.0f}M characters")
    run = 0
    for i, (s, c) in enumerate(chars.most_common(args.top), 1):
        run += c
        print(f"{i:>3}  {s:<32}{pages[s]:>8} pages {c / 1e6:>8.1f}M  {c / total:6.2%}  cumulative {run / total:6.1%}")


if __name__ == "__main__":
    main()
