"""Clean extracted documents into sentences and measure China-usage density.

Usage:
    uv run python pipeline/clean.py EXTRACTED_DIR CLEAN_DIR

Input : EXTRACTED_DIR/part-*.jsonl.gz   {"id","source","title","text"}
Output: CLEAN_DIR/part-*.jsonl.gz       {"id","source","cn_density","cn_weak","sents":[...]}
        CLEAN_DIR/part-*.stats.json     per-shard statistics
        CLEAN_DIR/part-*.hashes         64-bit document hashes (for de-duplication)

Streaming / resume: one input shard is processed at a time, line by line.
An output shard is written to a .tmp name and renamed when complete; shards
that already exist are skipped, and their document hashes are reloaded so
de-duplication still works across restarts.

cn_density = strong China-usage term hits per 1,000 CJK characters.
"""

from __future__ import annotations

import argparse
import array
import gzip
import hashlib
import json
import os
import re
from collections import Counter
from pathlib import Path

TERMS_FILE = Path(__file__).parent / "data" / "cn_terms.tsv"

MIN_CJK = 4          # minimum CJK characters in a sentence
MIN_CJK_RATIO = 0.7  # CJK characters / non-space characters
MAX_SENT_LEN = 200   # longer "sentences" are usually lists or tables

_RE_SPLIT = re.compile(r"[。！？!?；;\n\r…]+")
_RE_CJK = re.compile(r"[一-鿿㐀-䶿]")
_RE_SPACE_IN_CJK = re.compile(r"(?<=[一-鿿，、：「」（）])[ \t　]+(?=[一-鿿，、：「」（）])")
_RE_SPACES = re.compile(r"[ \t　\xa0]+")
_RE_URL = re.compile(r"https?://\S+|www\.\S+")
_RE_BOPOMOFO_OR_KANA = re.compile(r"[぀-ヿ가-힯]")
# Full-width ASCII letters/digits -> half-width. Punctuation is left alone.
_FW = {c: c - 0xFEE0 for c in list(range(0xFF10, 0xFF1A)) + list(range(0xFF21, 0xFF3B)) + list(range(0xFF41, 0xFF5B))}
_STRIP = " \t　，、：:,.-—─‧·*#>|"


def load_terms() -> tuple[re.Pattern, dict[str, str]]:
    strength: dict[str, str] = {}
    for line in TERMS_FILE.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        term, _tw, level = line.split("\t")
        strength[term] = level
    # Longest first so e.g. 打印機 wins over 打印.
    alt = "|".join(re.escape(t) for t in sorted(strength, key=len, reverse=True))
    return re.compile(alt), strength


def split_sentences(text: str) -> list[str]:
    text = _RE_URL.sub(" ", text.translate(_FW))
    out = []
    for raw in _RE_SPLIT.split(text):
        s = _RE_SPACES.sub(" ", _RE_SPACE_IN_CJK.sub("", raw)).strip(_STRIP)
        if not s or len(s) > MAX_SENT_LEN:
            continue
        cjk = len(_RE_CJK.findall(s))
        if cjk < MIN_CJK or cjk / len(s.replace(" ", "")) < MIN_CJK_RATIO:
            continue
        if _RE_BOPOMOFO_OR_KANA.search(s):
            continue
        out.append(s)
    return out


def doc_hash(sents: list[str]) -> int:
    h = hashlib.blake2b("\n".join(sents).encode("utf-8"), digest_size=8).digest()
    return int.from_bytes(h, "little")


def process_shard(src: Path, dst_dir: Path, seen: set[int], term_re, strength) -> dict:
    stem = src.name.removesuffix(".jsonl.gz")
    stats = Counter()
    term_hits: Counter = Counter()
    density_hist: Counter = Counter()
    hashes = array.array("Q")
    tmp = dst_dir / f"{stem}.tmp"
    with gzip.open(src, "rt", encoding="utf-8") as fin, \
            gzip.open(tmp, "wt", encoding="utf-8", compresslevel=5) as fout:
        for line in fin:
            doc = json.loads(line)
            stats["docs_in"] += 1
            sents = split_sentences(doc["text"])
            if not sents:
                stats["docs_empty"] += 1
                continue
            h = doc_hash(sents)
            if h in seen:
                stats["docs_duplicate"] += 1
                continue
            seen.add(h)
            hashes.append(h)

            joined = "\n".join(sents)
            cjk = len(_RE_CJK.findall(joined))
            strong = weak = 0
            for m in term_re.finditer(joined):
                t = m.group(0)
                term_hits[t] += 1
                if strength[t] == "strong":
                    strong += 1
                else:
                    weak += 1
            density = round(strong * 1000 / cjk, 3)
            bucket = "0" if strong == 0 else "<0.5" if density < 0.5 else "<1" if density < 1 \
                else "<2" if density < 2 else "<5" if density < 5 else ">=5"
            density_hist[bucket] += 1
            stats["docs_out"] += 1
            stats["sentences"] += len(sents)
            stats["cjk_chars"] += cjk
            fout.write(json.dumps({"id": doc["id"], "source": doc["source"],
                                   "cn_density": density, "cn_weak": weak, "sents": sents},
                                  ensure_ascii=False) + "\n")
    result = {**stats, "density_hist": dict(density_hist), "term_hits": dict(term_hits)}
    with open(dst_dir / f"{stem}.hashes", "wb") as fh:
        hashes.tofile(fh)
    (dst_dir / f"{stem}.stats.json").write_text(json.dumps(result, ensure_ascii=False))
    os.replace(tmp, dst_dir / f"{stem}.jsonl.gz")  # last: marks the shard complete
    return result


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("extracted_dir", type=Path)
    ap.add_argument("clean_dir", type=Path)
    args = ap.parse_args()
    args.clean_dir.mkdir(parents=True, exist_ok=True)
    term_re, strength = load_terms()

    seen: set[int] = set()
    todo = []
    for src in sorted(args.extracted_dir.glob("part-*.jsonl.gz")):
        stem = src.name.removesuffix(".jsonl.gz")
        if (args.clean_dir / f"{stem}.jsonl.gz").exists():
            a = array.array("Q")
            with open(args.clean_dir / f"{stem}.hashes", "rb") as fh:
                a.frombytes(fh.read())
            seen.update(a)
        else:
            todo.append(src)
    print(f"{len(todo)} shards to clean, {len(seen)} known document hashes", flush=True)
    for src in todo:
        r = process_shard(src, args.clean_dir, seen, term_re, strength)
        print(f"{src.name}: {r.get('docs_out', 0)}/{r.get('docs_in', 0)} docs, "
              f"{r.get('sentences', 0)} sentences, {r.get('cjk_chars', 0)} CJK chars", flush=True)


if __name__ == "__main__":
    main()
