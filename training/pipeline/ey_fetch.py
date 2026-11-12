"""Fetch Executive Yuan open-data text datasets and convert them to a JSONL shard.

Source: 行政院全球資訊網 開放資料集 https://www.ey.gov.tw/opendata/
        Licence: 政府資料開放授權條款－第1版 (attribution required)

Usage:
    uv run python pipeline/ey_fetch.py RAW_DIR OUT_DIR

Each dataset is one small JSON file (a few MB, most recent ~500 items), so
this finishes in under a minute; files already present in RAW_DIR are reused.
Output: OUT_DIR/part-ey.jsonl.gz   {"id","source","title","text"}
"""

from __future__ import annotations

import argparse
import gzip
import html
import json
import os
import re
import time
import urllib.request
from pathlib import Path

URL = "https://www.ey.gov.tw/NewOpenData/json/{id}"
USER_AGENT = "Mozilla/5.0 (compatible; research-crawler)"
# Dataset ids with running Chinese text (titles from the open-data catalogue).
DATASETS = {
    154: "行政院即時新聞",
    34: "重要政策",
    153: "行政院院會決議",
    40: "院會議案",
    185: "消費者保護新聞稿",
    74: "南部聯合服務中心-中心新聞",
    88: "雲嘉南區聯合服務中心-焦點新聞",
    51: "東部聯合服務中心-焦點新聞",
    50: "東部聯合服務中心-工作概況",
    198: "中部聯合服務中心-工作概況",
}
_RE_TAG = re.compile(r"<[^>]+>")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("raw_dir", type=Path)
    ap.add_argument("out_dir", type=Path)
    args = ap.parse_args()
    args.raw_dir.mkdir(parents=True, exist_ok=True)
    args.out_dir.mkdir(parents=True, exist_ok=True)

    tmp = args.out_dir / "part-ey.tmp"
    total = 0
    with gzip.open(tmp, "wt", encoding="utf-8") as out:
        for ds_id, name in DATASETS.items():
            raw = args.raw_dir / f"{ds_id}.json"
            if not raw.exists():
                req = urllib.request.Request(URL.format(id=ds_id), headers={"User-Agent": USER_AGENT})
                try:
                    with urllib.request.urlopen(req, timeout=120) as resp:
                        raw.write_bytes(resp.read())
                except Exception as e:
                    print(f"{ds_id} {name}: fetch failed: {e!r}", flush=True)
                    continue
                time.sleep(2)
            try:
                rows = json.loads(raw.read_text(encoding="utf-8-sig"))
            except ValueError:
                print(f"{ds_id} {name}: not JSON, skipped", flush=True)
                raw.unlink()
                continue
            n = 0
            for i, row in enumerate(rows if isinstance(rows, list) else []):
                text = html.unescape(_RE_TAG.sub("\n", str(row.get("內容") or "")))
                if len(text) < 50:
                    continue
                n += 1
                out.write(json.dumps({"id": f"ey-{ds_id}-{i}", "source": "ey",
                                      "title": str(row.get("標題") or ""), "text": text},
                                     ensure_ascii=False) + "\n")
            total += n
            print(f"{ds_id} {name}: {n} documents", flush=True)
    os.replace(tmp, args.out_dir / "part-ey.jsonl.gz")
    print(f"total {total} documents")


if __name__ == "__main__":
    main()
