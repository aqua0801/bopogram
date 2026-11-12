"""Stream a MediaWiki pages-articles dump (.xml.bz2) into plain-text JSONL shards.

Usage:
    uv run python pipeline/wiki_extract.py DUMP.xml.bz2 OUT_DIR --source zhwiki

Output: OUT_DIR/part-00000.jsonl.gz, ... one JSON object per article:
    {"id": ..., "source": ..., "title": ..., "text": ...}
Text is converted to Taiwan Traditional Chinese (OpenCC s2twp).

Streaming / resume:
  * The dump is decompressed and parsed incrementally; memory stays bounded
    (at most WINDOW batches of pages are in flight).
  * Every shard covers exactly PAGES_PER_SHARD main-namespace pages and is
    written atomically (tmp file + rename). On restart, finished shards are
    counted and that many pages are skipped without being parsed.
"""

from __future__ import annotations

import argparse
import bz2
import gzip
import json
import os
import re
import sys
import time
import xml.etree.ElementTree as ET
from collections import deque
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

PAGES_PER_SHARD = 20_000
BATCH = 100  # pages per worker task
WINDOW = 64  # max in-flight batches

# Variant preference inside -{ ... }- language-conversion markup.
VARIANT_ORDER = ("zh-tw", "zh-hant", "zh-hk", "zh-mo", "zh")

_RE_VARIANT = re.compile(r"-\{(.*?)\}-", re.S)
_RE_COMMENT = re.compile(r"<!--.*?-->", re.S)
_RE_REF = re.compile(r"<ref[^>/]*/>|<ref[^>]*>.*?</ref>", re.S | re.I)
_RE_BLOCK_TAGS = re.compile(
    r"<(gallery|math|chem|syntaxhighlight|source|timeline|score|pre|code|nowiki)[^>]*>.*?</\1>",
    re.S | re.I,
)
_RE_TABLE = re.compile(r"\{\|(?:(?!\{\|).)*?\|\}", re.S)
_RE_NS_LINK = re.compile(
    r"\[\[(?:File|Image|Category|檔案|文件|档案|圖像|图像|分類|分类)\s*:[^\[\]]*(?:\[\[[^\[\]]*\]\][^\[\]]*)*\]\]",
    re.I,
)
_RE_TAG = re.compile(r"<[^>]+>")
_RE_HEADING = re.compile(r"^=+\s*(.*?)\s*=+\s*$")
# Sections after which nothing useful follows.
_TAIL_SECTIONS = {
    "參考文獻", "参考文献", "參考資料", "参考资料", "外部連結", "外部链接",
    "參見", "参见", "注釋", "注释", "註釋", "腳註", "脚注", "參考", "参考",
    "相關條目", "相关条目", "延伸閱讀", "延伸阅读", "來源", "来源", "資料來源",
}

_cc = None


def _pick_variant(m: re.Match) -> str:
    body = m.group(1)
    if "|" in body:  # flags such as -{H|...}- / -{T|...}- produce no inline text
        flags, _, rest = body.partition("|")
        if any(f in flags for f in "HhTtDd-"):
            return ""
        body = rest
    if ":" not in body:
        return body
    found: dict[str, str] = {}
    for part in body.split(";"):
        key, sep, val = part.partition(":")
        if sep:
            found[key.strip().lower()] = val.strip()
    for v in VARIANT_ORDER:
        if v in found:
            return found[v]
    return next(iter(found.values()), "")


def clean_wikitext(text: str) -> str:
    import mwparserfromhell

    text = _RE_COMMENT.sub("", text)
    text = _RE_VARIANT.sub(_pick_variant, text)
    text = _RE_REF.sub("", text)
    text = _RE_BLOCK_TAGS.sub("", text)
    for _ in range(4):  # nested tables
        new = _RE_TABLE.sub("", text)
        if new == text:
            break
        text = new
    text = _RE_NS_LINK.sub("", text)
    try:
        text = mwparserfromhell.parse(text).strip_code(normalize=True, collapse=True)
    except Exception:
        return ""
    text = _RE_TAG.sub("", text)

    lines = []
    for line in text.split("\n"):
        line = line.strip()
        if not line:
            continue
        h = _RE_HEADING.match(line)
        if h:
            if h.group(1) in _TAIL_SECTIONS:
                break
            continue
        if line[0] in "|!{}*#:;" and len(line) < 20:
            continue
        lines.append(line.lstrip("*#:; "))
    return "\n".join(lines)


def _process_batch(batch: list[tuple[str, str, str]], source: str) -> list[str]:
    global _cc
    if _cc is None:
        import opencc

        _cc = opencc.OpenCC("s2twp")
    out = []
    for page_id, title, wikitext in batch:
        text = clean_wikitext(wikitext)
        if len(text) < 50:
            out.append("")  # keep position so shard boundaries stay deterministic
            continue
        out.append(
            json.dumps(
                {
                    "id": page_id,
                    "source": source,
                    "title": _cc.convert(title),
                    "text": _cc.convert(text),
                },
                ensure_ascii=False,
            )
        )
    return out


def iter_pages(dump_path: Path, skip: int):
    """Yield (id, title, wikitext) for main-namespace, non-redirect pages.

    The first `skip` qualifying pages are passed over without keeping text.
    """
    seen = 0
    with bz2.open(dump_path, "rb") as fh:
        ctx = ET.iterparse(fh, events=("start", "end"))
        _, root = next(ctx)
        ns = root.tag[: root.tag.index("}") + 1] if root.tag.startswith("{") else ""
        for event, elem in ctx:
            if event != "end" or elem.tag != ns + "page":
                continue
            try:
                if elem.findtext(ns + "ns") != "0" or elem.find(ns + "redirect") is not None:
                    continue
                seen += 1
                if seen <= skip:
                    continue
                yield (
                    elem.findtext(ns + "id") or "",
                    elem.findtext(ns + "title") or "",
                    elem.findtext(f"{ns}revision/{ns}text") or "",
                )
            finally:
                root.clear()  # free everything parsed so far


def write_shard(out_dir: Path, index: int, lines: list[str]) -> None:
    final = out_dir / f"part-{index:05d}.jsonl.gz"
    tmp = final.with_suffix(".tmp")
    with gzip.open(tmp, "wt", encoding="utf-8", compresslevel=5) as fh:
        for line in lines:
            if line:
                fh.write(line + "\n")
    os.replace(tmp, final)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("dump", type=Path)
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--source", required=True, help="source tag, e.g. zhwiki")
    ap.add_argument("--workers", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    ap.add_argument("--limit", type=int, default=0, help="stop after N pages (testing)")
    args = ap.parse_args()

    args.out_dir.mkdir(parents=True, exist_ok=True)
    for stale in args.out_dir.glob("*.tmp"):
        stale.unlink()
    done_shards = len(list(args.out_dir.glob("part-*.jsonl.gz")))
    if (args.out_dir / "_DONE").exists():
        print(f"already complete: {done_shards} shards")
        return
    skip = done_shards * PAGES_PER_SHARD
    if skip:
        print(f"resuming: {done_shards} shards done, skipping {skip} pages", flush=True)

    shard_idx = done_shards
    shard_lines: list[str] = []
    pages = kept = 0
    t0 = time.time()

    def drain(fut) -> None:
        nonlocal shard_idx, shard_lines, pages, kept
        for line in fut.result():
            shard_lines.append(line)
            pages += 1
            kept += bool(line)
            if len(shard_lines) == PAGES_PER_SHARD:
                write_shard(args.out_dir, shard_idx, shard_lines)
                shard_idx += 1
                shard_lines = []
                rate = pages / (time.time() - t0)
                print(f"shard {shard_idx} written; {skip + pages} pages, "
                      f"{kept} kept this run, {rate:.0f} pages/s", flush=True)

    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        pending: deque = deque()
        batch: list[tuple[str, str, str]] = []
        n = 0
        for page in iter_pages(args.dump, skip):
            batch.append(page)
            n += 1
            if len(batch) == BATCH:
                pending.append(pool.submit(_process_batch, batch, args.source))
                batch = []
                while len(pending) >= WINDOW:
                    drain(pending.popleft())
            if args.limit and n >= args.limit:
                break
        if batch:
            pending.append(pool.submit(_process_batch, batch, args.source))
        while pending:
            drain(pending.popleft())

    if shard_lines:
        write_shard(args.out_dir, shard_idx, shard_lines)
    if not args.limit:
        (args.out_dir / "_DONE").write_text(f"pages={skip + pages}\n")
    print(f"finished: {skip + pages} pages total, {kept} kept this run, "
          f"{time.time() - t0:.0f}s", file=sys.stderr)


if __name__ == "__main__":
    main()
