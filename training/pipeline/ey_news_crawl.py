"""Crawl Executive Yuan press releases (本院新聞) into JSONL shards.

Source: https://www.ey.gov.tw/Page/6485009ABEC1CB9C
        Licence: 政府資料開放授權條款－第1版 (attribution required).
        robots.txt allows these paths. One request per --delay seconds.

Usage:
    uv run python pipeline/ey_news_crawl.py STATE_DIR OUT_DIR [--delay 1.5] [--limit N]

Resume:
  * STATE_DIR/urls.txt  - article URLs, built once from the list pages
                          (STATE_DIR/urls.DONE marks completion; list-page
                          progress is kept in urls.partial + list_page.txt).
  * OUT_DIR/part-NNNNN.jsonl.gz - 500 URLs per shard, in urls.txt order,
    written atomically. Finished shards are skipped on restart.
Output record: {"id","source","title","text"}
"""

from __future__ import annotations

import argparse
import gzip
import html
import json
import os
import re
import sys
import time
import urllib.request
from pathlib import Path

BASE = "https://www.ey.gov.tw"
LIST_URL = BASE + "/Page/6485009ABEC1CB9C?page={page}&PS=100"
USER_AGENT = "Mozilla/5.0 (compatible; research-crawler)"
SHARD = 500

_RE_LINK = re.compile(r'href="(?:https://www\.ey\.gov\.tw)?(/Page/(?:9277F759E41CCD91|AF73D471993DF350)/[0-9a-f-]{36})"')
_RE_PAGES = re.compile(r"共(\d+)頁")
_RE_TITLE = re.compile(r'<span class="h2">\s*(.*?)\s*</span>', re.S)
_RE_BODY = re.compile(r'<div class="words_content[^"]*"[^>]*>(.*?)<div class="col-4 right_content"', re.S)
_RE_TAG = re.compile(r"<[^>]+>")
_RE_SCRIPT = re.compile(r"<(script|style)[^>]*>.*?</\1>", re.S | re.I)


def fetch(url: str, tries: int = 4) -> str:
    for attempt in range(tries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
            with urllib.request.urlopen(req, timeout=60) as resp:
                return resp.read().decode("utf-8", errors="replace")
        except Exception:
            if attempt == tries - 1:
                raise
            time.sleep(20 * (attempt + 1))
    raise AssertionError("unreachable")


def build_url_list(state: Path, delay: float) -> list[str]:
    final = state / "urls.txt"
    if (state / "urls.DONE").exists():
        return final.read_text().split()
    partial = state / "urls.partial"
    marker = state / "list_page.txt"
    page = int(marker.read_text()) + 1 if marker.exists() else 1
    total_pages = None
    with open(partial, "a", encoding="utf-8") as fh:
        while total_pages is None or page <= total_pages:
            body = fetch(LIST_URL.format(page=page))
            if m := _RE_PAGES.search(body):
                total_pages = int(m.group(1))
            links = list(dict.fromkeys(_RE_LINK.findall(body)))
            if not links:
                break
            fh.write("".join(link + "\n" for link in links))
            fh.flush()
            marker.write_text(str(page))
            if page % 20 == 0:
                print(f"list page {page}/{total_pages}", flush=True)
            page += 1
            time.sleep(delay)
    urls = list(dict.fromkeys(partial.read_text().split()))
    final.write_text("\n".join(urls) + "\n")
    (state / "urls.DONE").write_text(f"{len(urls)}\n")
    return urls


def parse_article(body: str) -> tuple[str, str]:
    title = _RE_TITLE.search(body)
    content = _RE_BODY.search(body)
    if not content:
        return "", ""
    text = html.unescape(_RE_TAG.sub("\n", _RE_SCRIPT.sub("", content.group(1))))
    text = re.sub(r"\s*\n\s*", "\n", text).strip()
    return (html.unescape(_RE_TAG.sub("", title.group(1))).strip() if title else ""), text


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("state_dir", type=Path)
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--delay", type=float, default=1.5)
    ap.add_argument("--limit", type=int, default=0, help="stop after N shards (testing)")
    args = ap.parse_args()
    args.state_dir.mkdir(parents=True, exist_ok=True)
    args.out_dir.mkdir(parents=True, exist_ok=True)

    urls = build_url_list(args.state_dir, args.delay)
    n_shards = (len(urls) + SHARD - 1) // SHARD
    print(f"{len(urls)} article urls, {n_shards} shards", flush=True)
    done_now = 0
    for idx in range(n_shards):
        final = args.out_dir / f"part-{idx:05d}.jsonl.gz"
        if final.exists():
            continue
        tmp = final.with_suffix(".tmp")
        kept = failed = 0
        with gzip.open(tmp, "wt", encoding="utf-8") as fh:
            for path in urls[idx * SHARD:(idx + 1) * SHARD]:
                try:
                    title, text = parse_article(fetch(BASE + path))
                except Exception as e:
                    failed += 1
                    print(f"failed {path}: {e!r}", flush=True)
                    title, text = "", ""
                if len(text) >= 50:
                    kept += 1
                    fh.write(json.dumps({"id": path.rsplit("/", 1)[-1], "source": "ey_news",
                                         "title": title, "text": text}, ensure_ascii=False) + "\n")
                time.sleep(args.delay)
        os.replace(tmp, final)
        print(f"shard {idx + 1}/{n_shards}: kept {kept}, failed {failed}", flush=True)
        done_now += 1
        if args.limit and done_now >= args.limit:
            break
    print("finished", file=sys.stderr)


if __name__ == "__main__":
    main()
