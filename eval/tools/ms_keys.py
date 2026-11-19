"""Turn test sentences into Dachen keystrokes, for typing them with another IME.

Usage (WSL):
    uv run --project ../prototype python tools/ms_keys.py TESTSET_TSV READINGS_TSV OUT_TSV [--limit N]

Output: OUT_TSV with columns id, keys. The keys are what a person would press
on a Dachen keyboard: initial, medial, final, then the tone (Space for the
first tone, 7 for the neutral tone). tools/ms_type.py types them on Windows.
"""

from __future__ import annotations

import argparse
import csv
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "prototype"))
from bopogram.layouts.dachen import DACHEN  # noqa: E402

KEY = {sym: k for k, sym in DACHEN.keys.items()}
TONES = {"ˊ": "6", "ˇ": "3", "ˋ": "4"}


def syllable_keys(syl: str) -> str:
    neutral = syl.startswith("˙")
    body = syl.lstrip("˙")
    tone = " "
    if body and body[-1] in TONES:
        body, tone = body[:-1], TONES[body[-1]]
    if neutral:
        tone = "7"
    return "".join(KEY[ch] for ch in body) + tone


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("testset", type=Path)
    ap.add_argument("readings", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--limit", type=int)
    args = ap.parse_args()
    with open(args.readings, encoding="utf-8", newline="") as fh:
        readings = {r["id"]: r["bopomofo"].split() for r in csv.DictReader(fh, delimiter="\t")}
    with open(args.testset, encoding="utf-8", newline="") as fh:
        ids = [r["id"] for r in csv.DictReader(fh, delimiter="\t")]
    ids = ids[: args.limit] if args.limit else ids
    with open(args.out, "w", encoding="utf-8", newline="\n") as out:
        out.write("id\tkeys\n")
        for i in ids:
            out.write(f"{i}\t{''.join(syllable_keys(s) for s in readings[i])}\n")
    print(f"{len(ids)} sentences -> {args.out}")


if __name__ == "__main__":
    main()
