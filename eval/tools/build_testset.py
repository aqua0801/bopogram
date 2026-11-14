"""Assemble the draft test set and the development set.

Usage:
    python3 eval/tools/build_testset.py --dict DICT_TSV --moztw MOZTW_ZH_TW_DIR \
        [--corpus CORPUS_DIR]

Writes:
    eval/testset/testset_draft.tsv   id, category, context, text, note
    eval/devset/moztw_dev.tsv        same columns; used for tuning, never for reporting

Sources:
  * eval/tools/src_*.txt  sentences written for this project
    (line format: [context>>]text[|note])
  * MozTW cc0-sentences (CC0): a fixed random pick of chat-like sentences goes
    to the "daily" category of the test set; the remaining sentences form the
    development set. The two never overlap.

Checks: every character must have a reading in the dictionary; duplicates are
dropped. With --corpus, sentences that occur verbatim as a training sentence
get "〔語料重複〕" appended to the note so accuracy can be reported with and
without them.
"""

from __future__ import annotations

import argparse
import gzip
import random
import re
from pathlib import Path

HERE = Path(__file__).resolve().parent
EVAL = HERE.parent
CATEGORIES = [  # (source file, category, id prefix)
    ("src_homophone.txt", "homophone", "H"),
    ("src_polyphone.txt", "polyphone", "P"),
    ("src_twcn.txt", "tw_cn", "T"),
    ("src_long.txt", "long", "L"),
    ("src_longdist.txt", "longdist", "R"),
    ("src_daily.txt", "daily", "D"),
]
MOZTW_FILES = ["chatlogs.txt", "g0v_slack_rand0m.txt", "web_slang.txt", "setences.txt", "sayit.txt"]
MOZTW_TEST_PICKS = 60
CJK_ONLY = re.compile(r"^[一-鿿]+$")
OVERLAP_MARK = "〔語料重複〕"


def read_src(path: Path) -> list[tuple[str, str, str]]:
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        body, _, note = line.partition("|")
        context, sep, text = body.partition(">>")
        if not sep:
            context, text = "", body
        rows.append((context.strip(), text.strip(), note.strip()))
    return rows


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--dict", type=Path, required=True)
    ap.add_argument("--moztw", type=Path, required=True)
    ap.add_argument("--corpus", type=Path)
    args = ap.parse_args()

    chars = set()
    with open(args.dict, encoding="utf-8") as fh:
        for line in fh:
            word = line.split("\t", 1)[0]
            if len(word) == 1:
                chars.add(word)

    def readable(text: str) -> bool:
        return bool(CJK_ONLY.match(text)) and all(c in chars for c in text)

    test: list[list[str]] = []
    seen: set[str] = set()
    problems: list[str] = []
    for fname, category, prefix in CATEGORIES:
        n = 0
        for context, text, note in read_src(HERE / fname):
            if text in seen:
                problems.append(f"duplicate dropped: {text}")
                continue
            if not readable(text) or (context and not readable(context)):
                problems.append(f"unreadable character in: {text}")
                continue
            seen.add(text)
            n += 1
            test.append([f"{prefix}{n:03d}", category, context, text, note])

    pool: list[tuple[str, str]] = []
    for fname in MOZTW_FILES:
        path = args.moztw / fname
        if not path.exists():
            continue
        for line in path.read_text(encoding="utf-8").splitlines():
            text = line.strip()
            if 4 <= len(text) <= 18 and readable(text) and text not in seen:
                seen.add(text)
                pool.append((text, fname.removesuffix(".txt")))
    random.Random(20261114).shuffle(pool)
    n_daily = sum(1 for row in test if row[1] == "daily")
    for i, (text, origin) in enumerate(pool[:MOZTW_TEST_PICKS], 1):
        test.append([f"D{n_daily + i:03d}", "daily", "", text, f"MozTW:{origin}"])
    dev = [[f"V{i:05d}", "daily", "", text, f"MozTW:{origin}"]
           for i, (text, origin) in enumerate(pool[MOZTW_TEST_PICKS:], 1)]

    if args.corpus:
        wanted = {row[3] for row in test}
        found: set[str] = set()
        for shard in sorted(args.corpus.glob("*/part-*.txt.gz")):
            with gzip.open(shard, "rt", encoding="utf-8") as fh:
                for line in fh:
                    s = line.rstrip("\n")
                    if s in wanted:
                        found.add(s)
        for row in test:
            if row[3] in found:
                row[4] = (row[4] + OVERLAP_MARK) if row[4] else OVERLAP_MARK
        print(f"verbatim overlap with training corpus: {len(found)} of {len(test)} sentences")

    header = "id\tcategory\tcontext\ttext\tnote\n"
    (EVAL / "testset").mkdir(exist_ok=True)
    (EVAL / "devset").mkdir(exist_ok=True)
    with open(EVAL / "testset" / "testset_draft.tsv", "w", encoding="utf-8") as fh:
        fh.write(header + "".join("\t".join(r) + "\n" for r in test))
    with open(EVAL / "devset" / "moztw_dev.tsv", "w", encoding="utf-8") as fh:
        fh.write(header + "".join("\t".join(r) + "\n" for r in dev))

    by_cat: dict[str, int] = {}
    for row in test:
        by_cat[row[1]] = by_cat.get(row[1], 0) + 1
    print("test set:", by_cat, "total", len(test))
    print("dev set:", len(dev))
    for p in problems:
        print("  !", p)


if __name__ == "__main__":
    main()
