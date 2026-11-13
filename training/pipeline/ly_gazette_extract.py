"""Convert downloaded Legislative Yuan gazette .doc files into JSONL shards.

Usage:
    uv run python pipeline/ly_gazette_extract.py RAW_DIR OUT_DIR

Input : RAW_DIR/doc/<year>/*.doc   (from ly_gazette_fetch.py)
Output: OUT_DIR/part-<year>.jsonl.gz   {"id","source","title","text"}

Requires the external `antiword` command. One .doc is converted at a time.
Resume: one shard per year directory, written atomically; years whose shard
already exists are skipped. Run this after the download has finished (a year
converted while its files are still arriving would be incomplete).
"""

from __future__ import annotations

import argparse
import gzip
import html
import json
import os
import re
import subprocess
import sys
import zipfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

# "主席：", "王委員小明：", "陳部長建仁：" ... speaker label at line start.
_RE_SPEAKER = re.compile(r"^[一-鿿‧·．\s（）()]{1,18}：")
_RE_TABLE_LINE = re.compile(r"^\s*\|")
_RE_PAGE_HEADER = re.compile(r"^立法院公報\s*第.*卷.*期|^\s*\d+\s*$")


_RE_DOCX_PARA = re.compile(r"<w:p[ >].*?</w:p>", re.S)
_RE_DOCX_TEXT = re.compile(r"<w:t(?: [^>]*)?>([^<]*)</w:t>")
_RE_DOCX_TABLE = re.compile(r"<w:tbl>.*?</w:tbl>", re.S)


def docx_to_lines(path: Path) -> list[str]:
    """Plain paragraphs of a .docx (tables dropped), using only the stdlib."""
    try:
        with zipfile.ZipFile(path) as zf:
            xml = zf.read("word/document.xml").decode("utf-8", errors="replace")
    except (zipfile.BadZipFile, KeyError, OSError):
        return []
    xml = _RE_DOCX_TABLE.sub("", xml)
    return [html.unescape("".join(_RE_DOCX_TEXT.findall(p))) for p in _RE_DOCX_PARA.findall(xml)]


def doc_to_text(path: Path) -> str:
    with open(path, "rb") as fh:
        is_docx = fh.read(4) == b"PK\x03\x04"
    if is_docx:
        raw_lines = docx_to_lines(path)
    else:
        raw_lines = antiword_lines(path)
    lines = []
    for line in raw_lines:
        if _RE_TABLE_LINE.match(line) or _RE_PAGE_HEADER.match(line):
            continue
        line = _RE_SPEAKER.sub("", line.strip(), count=1).strip()
        if line:
            lines.append(line)
    return "\n".join(lines)


def antiword_lines(path: Path) -> list[str]:
    try:
        res = subprocess.run(
            ["antiword", "-m", "UTF-8.txt", "-w", "0", str(path)],
            capture_output=True, timeout=300,
        )
    except subprocess.TimeoutExpired:
        return []
    if res.returncode != 0:
        return []
    return res.stdout.decode("utf-8", errors="replace").split("\n")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("raw_dir", type=Path)
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--workers", type=int, default=8)
    args = ap.parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)

    for year_dir in sorted(p for p in (args.raw_dir / "doc").iterdir() if p.is_dir()):
        final = args.out_dir / f"part-{year_dir.name}.jsonl.gz"
        if final.exists():
            continue
        files = sorted(year_dir.glob("*.doc"))
        tmp = final.with_suffix(".tmp")
        kept = chars = 0
        with ThreadPoolExecutor(args.workers) as pool, \
                gzip.open(tmp, "wt", encoding="utf-8", compresslevel=5) as fh:
            # map() yields lazily in order; antiword runs as separate processes.
            for path, text in zip(files, pool.map(doc_to_text, files)):
                if len(text) < 50:
                    continue
                kept += 1
                chars += len(text)
                fh.write(json.dumps({"id": path.stem, "source": "ly_gazette",
                                     "title": path.stem, "text": text},
                                    ensure_ascii=False) + "\n")
        os.replace(tmp, final)
        print(f"{year_dir.name}: {kept}/{len(files)} files, {chars / 1e6:.1f}M chars", flush=True)
    print("finished", file=sys.stderr)


if __name__ == "__main__":
    main()
