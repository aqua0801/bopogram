"""Download Legislative Yuan gazette (公報) source .doc files.

Source: 立法院開放資料服務平台, dataset 41 "公報原始檔案"
        https://data.ly.gov.tw/getds.action?id=41  (open licence, attribution required)

Usage:
    uv run python pipeline/ly_gazette_fetch.py OUT_DIR [--delay 2.0] [--limit N]

Steps (both resumable):
  1. Index: fetch the paginated JSON index into OUT_DIR/index.jsonl.
     Skipped when OUT_DIR/index.DONE exists.
  2. Files: download every unique docUrl into OUT_DIR/doc/<year>/<name>.doc,
     one request per --delay seconds. Existing files are skipped; each file
     is written to a .tmp name and renamed, so a kill never leaves a partial
     file behind. Failures are appended to OUT_DIR/failed.tsv and retried on
     the next run.
     Files larger than --max-mb are not downloaded: they are dominated by
     embedded images (a 129 MB file held 68K characters of text). Their URLs
     are listed in OUT_DIR/skipped_large.tsv and not requested again.
"""

from __future__ import annotations

import argparse
import json
import os
import ssl
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

INDEX_URL = "https://data.ly.gov.tw/odw/openDatasetJson.action?id=41&selectTerm=all&page={page}"
USER_AGENT = "Mozilla/5.0 (compatible; research-crawler)"
OLE_MAGIC = b"\xd0\xcf\x11\xe0"


def make_context() -> ssl.SSLContext:
    ctx = ssl.create_default_context()
    # data.ly.gov.tw only supports legacy TLS renegotiation.
    ctx.options |= getattr(ssl, "OP_LEGACY_SERVER_CONNECT", 0x4)
    return ctx


class TooLarge(Exception):
    pass


def fetch(url: str, ctx: ssl.SSLContext, timeout: int = 120, max_bytes: int = 0) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, context=ctx, timeout=timeout) as resp:
        size = int(resp.headers.get("Content-Length") or 0)
        if max_bytes and size > max_bytes:
            raise TooLarge(str(size))
        if not max_bytes:
            return resp.read()
        body = resp.read(max_bytes + 1)  # also guards responses without a length header
        if len(body) > max_bytes:
            raise TooLarge(f">{max_bytes}")
        return body


def fetch_retry(url: str, ctx: ssl.SSLContext, tries: int = 4, max_bytes: int = 0) -> bytes:
    for attempt in range(tries):
        try:
            return fetch(url, ctx, max_bytes=max_bytes)
        except TooLarge:
            raise
        except urllib.error.HTTPError as e:
            if e.code in (403, 404, 410):
                raise
            err: Exception = e
        except Exception as e:  # network errors, timeouts
            err = e
        if attempt == tries - 1:
            raise err
        time.sleep(15 * (attempt + 1))
    raise AssertionError("unreachable")


def build_index(out_dir: Path, ctx: ssl.SSLContext, delay: float) -> None:
    done = out_dir / "index.DONE"
    if done.exists():
        return
    tmp = out_dir / "index.jsonl.tmp"
    rows = 0
    with open(tmp, "w", encoding="utf-8") as fh:
        page = 1
        while True:
            data = json.loads(fetch_retry(INDEX_URL.format(page=page), ctx))
            batch = data.get("jsonList") or []
            if not batch:
                break
            for row in batch:
                fh.write(json.dumps(row, ensure_ascii=False) + "\n")
            rows += len(batch)
            print(f"index page {page}: {len(batch)} rows (total {rows})", flush=True)
            page += 1
            time.sleep(delay)
    os.replace(tmp, out_dir / "index.jsonl")
    done.write_text(f"rows={rows}\n")


def unique_urls(out_dir: Path) -> list[str]:
    seen: dict[str, None] = {}
    with open(out_dir / "index.jsonl", encoding="utf-8") as fh:
        for line in fh:
            url = (json.loads(line).get("docUrl") or "").strip()
            if url.startswith("https://") and url.lower().endswith(".doc"):
                seen.setdefault(url)
    return list(seen)


def local_path(out_dir: Path, url: str) -> Path:
    parts = url.split("/")
    # .../communique1/work/<year>/<volume>/<name>.doc
    year = parts[-3] if len(parts) >= 3 and parts[-3].isdigit() else "misc"
    return out_dir / "doc" / year / parts[-1]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--delay", type=float, default=2.0, help="seconds between requests")
    ap.add_argument("--limit", type=int, default=0, help="download at most N files (testing)")
    ap.add_argument("--max-mb", type=float, default=30.0, help="skip files larger than this")
    args = ap.parse_args()

    out_dir: Path = args.out_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    ctx = make_context()

    build_index(out_dir, ctx, args.delay)
    urls = unique_urls(out_dir)
    skip_file = out_dir / "skipped_large.tsv"
    skipped = set()
    if skip_file.exists():
        skipped = {line.split("\t")[0] for line in skip_file.read_text().splitlines()}
    max_bytes = int(args.max_mb * 1e6)
    todo = [u for u in urls if u not in skipped and not local_path(out_dir, u).exists()]
    print(f"{len(urls)} unique files, {len(urls) - len(todo)} already present, "
          f"{len(todo)} to download", flush=True)
    if args.limit:
        todo = todo[: args.limit]

    ok = failed = 0
    nbytes = 0
    t0 = time.time()
    with open(out_dir / "failed.tsv", "a", encoding="utf-8") as fail_log:
        for i, url in enumerate(todo, 1):
            dest = local_path(out_dir, url)
            dest.parent.mkdir(parents=True, exist_ok=True)
            try:
                body = fetch_retry(url, ctx, max_bytes=max_bytes)
                if not body.startswith(OLE_MAGIC):
                    raise ValueError(f"not a .doc file ({len(body)} bytes)")
                tmp = dest.with_suffix(".tmp")
                tmp.write_bytes(body)
                os.replace(tmp, dest)
                ok += 1
                nbytes += len(body)
            except TooLarge as e:
                with open(skip_file, "a", encoding="utf-8") as sf:
                    sf.write(f"{url}\t{e}\n")
            except Exception as e:
                failed += 1
                fail_log.write(f"{time.strftime('%F %T')}\t{url}\t{e!r}\n")
                fail_log.flush()
            if i % 100 == 0 or i == len(todo):
                el = time.time() - t0
                eta_h = (len(todo) - i) * (el / i) / 3600
                print(f"{i}/{len(todo)} ok={ok} failed={failed} "
                      f"{nbytes / 1e6:.0f}MB eta={eta_h:.1f}h", flush=True)
            time.sleep(args.delay)

    print(f"finished: ok={ok} failed={failed}", file=sys.stderr)


if __name__ == "__main__":
    main()
