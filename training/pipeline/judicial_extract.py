"""Convert Judicial Yuan monthly judgment archives (.rar) into JSONL shards.

Usage:
    uv run python pipeline/judicial_extract.py RAW_DIR OUT_DIR [--work-dir DIR]

Input : RAW_DIR/<YYYYMM>.rar, RAW_DIR/delete_infor.csv   (from judicial_fetch.py)
Output: OUT_DIR/part-<YYYYMM>.jsonl.gz   {"id","source","title","text"}

Requires the external `unar` command. Each archive is unpacked into a
scratch directory on disk, its JSON files are read one at a time, and the
scratch directory is removed afterwards. Judgments listed in
delete_infor.csv (withdrawn by the courts) are dropped.
Resume: one shard per archive, written atomically; existing shards are skipped.
"""

from __future__ import annotations

import argparse
import csv
import gzip
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

SHORT_LINE = 30  # lines shorter than this are headings / paragraph ends, not hard wraps


def load_deleted(path: Path) -> set[str]:
    if not path.exists():
        return set()
    with open(path, encoding="utf-8-sig", newline="") as fh:
        rows = csv.reader(fh)
        next(rows, None)
        return {row[3].strip() for row in rows if len(row) >= 4}


def unwrap(full: str) -> str:
    """Judgment text is hard-wrapped at a fixed width; re-join wrapped lines."""
    out: list[str] = []
    buf = ""
    for line in full.replace("\r", "").split("\n"):
        stripped = line.strip(" 　")
        if not stripped:
            if buf:
                out.append(buf)
                buf = ""
            continue
        buf += stripped
        if len(line) < SHORT_LINE:
            out.append(buf)
            buf = ""
    if buf:
        out.append(buf)
    return "\n".join(out)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("raw_dir", type=Path)
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--work-dir", type=Path, default=None,
                    help="scratch directory (default: OUT_DIR/_work)")
    args = ap.parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)
    work = args.work_dir or args.out_dir / "_work"
    deleted = load_deleted(args.raw_dir / "delete_infor.csv")
    print(f"{len(deleted)} withdrawn judgment ids loaded", flush=True)

    for rar in sorted(args.raw_dir.glob("*.rar")):
        final = args.out_dir / f"part-{rar.stem}.jsonl.gz"
        if final.exists():
            continue
        shutil.rmtree(work, ignore_errors=True)
        work.mkdir(parents=True)
        subprocess.run(["unar", "-q", "-o", str(work), str(rar)], check=True)
        tmp = final.with_suffix(".tmp")
        kept = dropped = chars = 0
        with gzip.open(tmp, "wt", encoding="utf-8", compresslevel=5) as fh:
            for path in sorted(work.rglob("*.json")):
                try:
                    doc = json.loads(path.read_text(encoding="utf-8-sig"))
                except (ValueError, UnicodeDecodeError):
                    dropped += 1
                    continue
                jid = doc.get("JID", "")
                text = unwrap(doc.get("JFULL") or "")
                if jid in deleted or len(text) < 50:
                    dropped += 1
                    continue
                kept += 1
                chars += len(text)
                fh.write(json.dumps({"id": jid, "source": "judicial",
                                     "title": doc.get("JTITLE", ""), "text": text},
                                    ensure_ascii=False) + "\n")
        os.replace(tmp, final)
        shutil.rmtree(work, ignore_errors=True)
        print(f"{rar.stem}: kept {kept}, dropped {dropped}, {chars / 1e6:.1f}M chars", flush=True)
    print("finished", file=sys.stderr)


if __name__ == "__main__":
    main()
