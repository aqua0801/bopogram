"""Turn the segmented corpus into (character, syllable) arrays for neural training.

Usage:
    uv run python neural/make_data.py vocab DICT_TSV OUT_DIR
    uv run python neural/make_data.py run SEGMENTED_DIR DICT_TSV COUNTS_TSV OUT_DIR [--workers N]
    uv run python neural/make_data.py sample OUT_DIR [--n 50] [--seed 1]

Input : SEGMENTED_DIR/<source>/part-*.txt.gz   tokens separated by spaces
Output: OUT_DIR/vocab.json                     character and syllable tables
        OUT_DIR/<source>/part-*.chars.npy      uint16 character id per position
        OUT_DIR/<source>/part-*.syls.npy       uint16 syllable id per position (0 = none)
        OUT_DIR/<source>/part-*.offs.npy       uint64 start of each sentence (+ end)

One position per character. A run of digits or Latin letters is one position
(<n> / <e>); a character without a dictionary reading is <x>. Positions that
are not readable Chinese characters have syllable 0: they can serve as left
context but are never predicted.

Streaming / resume: each shard is read line by line by one worker; arrays are
written under a .tmp name and renamed, .offs.npy last; shards whose .offs.npy
exists are skipped. The reading drawn for ambiguous words depends only on the
shard name, so a re-run gives the same data.
"""

from __future__ import annotations

import argparse
import gzip
import json
import os
import random
import re
import sys
import zlib
from array import array
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from readings import PREFERRED_FORMS, Readings  # noqa: E402

SPECIALS = ["<pad>", "<x>", "<n>", "<e>", "<p>", "<s>"]
PUNCT_KEPT = "，。？！、；：「」（）"
_RE_NUM = re.compile(r"^[0-9][0-9.%+\-]*$")
_RE_LATIN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9.%+\-]*$")
_RE_CJK = re.compile(r"^[一-鿿㐀-䶿]+$")

_readings: Readings | None = None
_char_id: dict[str, int] = {}
_syl_id: dict[str, int] = {}


def build_vocab(dict_tsv: Path, out_dir: Path) -> None:
    chars, syls = set(), set()
    cands: dict[str, set[str]] = {}
    with open(dict_tsv, encoding="utf-8") as fh:
        for line in fh:
            parts = line.rstrip("\n").split("\t")
            if len(parts) >= 2:
                reading = parts[1].split()
                if len(parts[0]) == 1:
                    chars.add(parts[0])
                syls.update(reading)
                if len(reading) == len(parts[0]):
                    for ch, syl in zip(parts[0], reading):
                        cands.setdefault(syl, set()).add(ch)
    # "candidates": every character the dictionary lets a syllable stand for
    # (alone or inside a word); the model only ever chooses among these.
    vocab = {"chars": SPECIALS + list(PUNCT_KEPT) + sorted(chars), "syllables": ["<none>"] + sorted(syls),
             "candidates": {s: "".join(sorted(c for c in cands.get(s, ()) if c in chars)) for s in sorted(syls)}}
    out_dir.mkdir(parents=True, exist_ok=True)
    tmp = out_dir / "vocab.json.tmp"
    tmp.write_text(json.dumps(vocab, ensure_ascii=False), encoding="utf-8")
    os.replace(tmp, out_dir / "vocab.json")
    print(f"{len(vocab['chars'])} characters, {len(vocab['syllables'])} syllables")


def load_vocab(out_dir: Path) -> dict:
    return json.loads((out_dir / "vocab.json").read_text(encoding="utf-8"))


def init_worker(dict_tsv: str, counts_tsv: str, out_dir: str) -> None:
    global _readings, _char_id, _syl_id
    _readings = Readings(Path(dict_tsv), Path(counts_tsv))
    vocab = load_vocab(Path(out_dir))
    _char_id = {c: i for i, c in enumerate(vocab["chars"])}
    _syl_id = {s: i for i, s in enumerate(vocab["syllables"])}


def encode_line(tokens: list[str], rng: random.Random, chars: array, syls: array) -> None:
    X, N, E, P = _char_id["<x>"], _char_id["<n>"], _char_id["<e>"], _char_id["<p>"]
    last_p = True  # drop leading punctuation and collapse repeated unknown punctuation
    for tok in tokens:
        tok = tok.translate(PREFERRED_FORMS)
        reading = _readings.read(tok, rng)
        if reading is not None and len(reading) == len(tok):
            for ch, syl in zip(tok, reading):
                cid = _char_id.get(ch)
                if cid is None:
                    chars.append(X)
                    syls.append(0)
                else:
                    chars.append(cid)
                    syls.append(_syl_id[syl])
            last_p = False
            continue
        if _RE_CJK.match(tok):  # no usable reading: context only
            for ch in tok:
                chars.append(_char_id.get(ch, X))
                syls.append(0)
            last_p = False
        elif _RE_NUM.match(tok):
            chars.append(N); syls.append(0); last_p = False
        elif _RE_LATIN.match(tok):
            chars.append(E); syls.append(0); last_p = False
        else:
            cid = _char_id.get(tok, P)
            if cid == P and last_p:
                continue
            chars.append(cid); syls.append(0)
            last_p = cid == P


def run_shard(job: tuple[str, str]) -> tuple[str, int, int]:
    src, stem = job
    rng = random.Random(zlib.crc32("/".join(Path(src).parts[-2:]).encode()))
    chars, syls, offs = array("H"), array("H"), array("Q")
    with gzip.open(src, "rt", encoding="utf-8") as fh:
        for line in fh:
            start = len(chars)
            encode_line(line.split(), rng, chars, syls)
            if len(chars) > start:
                offs.append(start)
    offs.append(len(chars))
    for name, arr, dtype in (("chars", chars, np.uint16), ("syls", syls, np.uint16), ("offs", offs, np.uint64)):
        tmp = f"{stem}.{name}.tmp.npy"
        np.save(tmp, np.frombuffer(arr, dtype=dtype))
        os.replace(tmp, f"{stem}.{name}.npy")
    return stem, len(offs) - 1, len(chars)


def sample(out_dir: Path, n: int, seed: int) -> None:
    vocab = load_vocab(out_dir)
    rng = random.Random(seed)
    shards = sorted(out_dir.glob("*/part-*.offs.npy"))
    for _ in range(n):
        offs_path = rng.choice(shards)
        stem = str(offs_path)[: -len(".offs.npy")]
        offs = np.load(offs_path, mmap_mode="r")
        chars = np.load(stem + ".chars.npy", mmap_mode="r")
        syls = np.load(stem + ".syls.npy", mmap_mode="r")
        k = rng.randrange(len(offs) - 1)
        a, b = int(offs[k]), int(offs[k + 1])
        text = "".join(vocab["chars"][c] for c in chars[a:b])
        reading = " ".join(vocab["syllables"][s] if s else "-" for s in syls[a:b])
        print(f"[{offs_path.parent.name}] {text}\n    {reading}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    v = sub.add_parser("vocab")
    v.add_argument("dict_tsv", type=Path)
    v.add_argument("out_dir", type=Path)
    r = sub.add_parser("run")
    r.add_argument("segmented_dir", type=Path)
    r.add_argument("dict_tsv", type=Path)
    r.add_argument("counts_tsv", type=Path)
    r.add_argument("out_dir", type=Path)
    r.add_argument("--workers", type=int, default=max(1, (os.cpu_count() or 2) - 4))
    r.add_argument("--only", help="process only shards whose path contains this text")
    s = sub.add_parser("sample")
    s.add_argument("out_dir", type=Path)
    s.add_argument("--n", type=int, default=50)
    s.add_argument("--seed", type=int, default=1)
    args = ap.parse_args()

    if args.cmd == "vocab":
        build_vocab(args.dict_tsv, args.out_dir)
        return
    if args.cmd == "sample":
        sample(args.out_dir, args.n, args.seed)
        return

    jobs = []
    for src in sorted(args.segmented_dir.glob("*/part-*.txt.gz")):
        if args.only and args.only not in str(src):
            continue
        dst_dir = args.out_dir / src.parent.name
        dst_dir.mkdir(parents=True, exist_ok=True)
        stem = str(dst_dir / src.name.removesuffix(".txt.gz"))
        if not os.path.exists(stem + ".offs.npy"):
            jobs.append((str(src), stem))
    print(f"{len(jobs)} shards to encode", flush=True)
    sentences = positions = 0
    with ProcessPoolExecutor(args.workers, initializer=init_worker,
                             initargs=(str(args.dict_tsv), str(args.counts_tsv), str(args.out_dir))) as pool:
        for k, (_, n_sent, n_pos) in enumerate(pool.map(run_shard, jobs), 1):
            sentences += n_sent
            positions += n_pos
            if k % 10 == 0 or k == len(jobs):
                print(f"{k}/{len(jobs)} shards, {sentences:,} sentences, {positions:,} positions", flush=True)


if __name__ == "__main__":
    main()
