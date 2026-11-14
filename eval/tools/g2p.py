"""Generate the bopomofo input for each test sentence.

Usage (from prototype/, so the engine package is importable):
    uv run python ../eval/tools/g2p.py TESTSET_TSV OUT_TSV [--report]

Output: id <TAB> syllables separated by spaces (one per character of `text`).

How a reading is chosen:
  1. The sentence is segmented into dictionary words.
  2. A word with one reading uses it. With several, variant readings (":var")
     are dropped first; the reading whose characters agree most with
     char_defaults.tsv wins, then the source priority concised > idiom >
     manual > web > revised.
  3. Single characters with several readings use eval/tools/char_defaults.tsv
     (the reading a typist would normally use for the character on its own).
  4. A note of the form "字=ㄓㄨˋ" forces that reading for that character.
The output is reviewed by hand. Corrections live in eval/tools/reading_fixes.tsv:
    id <TAB> substring=syllables     fix that substring in one sentence
    *  <TAB> substring=syllables     fix it wherever it occurs
Neutral tones: except for particles and the suffix 子, the citation-tone
spelling is preferred (名字 as ㄗˋ), because that is what most people type and
it is the harder case for the engine.
"""

from __future__ import annotations

import argparse
import csv
import re
import sys
from pathlib import Path

from bopogram.context import segment
from bopogram.engine import Engine

HERE = Path(__file__).resolve().parent
PRIORITY = {"concised": 0, "idiom": 1, "manual": 2, "web": 3, "revised": 4}
_RE_FORCE = re.compile(r"([一-鿿])=(˙?[ㄅ-ㄩ]{1,3}[ˊˇˋ]?)")


KEEP_NEUTRAL = set("的了著們嗎吧呢啊啦麼子")


def load_fixes(path: Path) -> list[tuple[str, str, list[str]]]:
    out = []
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines():
            if line and not line.startswith("#"):
                rid, _, rest = line.partition("\t")
                sub, _, syls = rest.partition("=")
                out.append((rid, sub, syls.split()))
    return out


def load_pairs(path: Path) -> dict[str, str]:
    out = {}
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines():
            if line and not line.startswith("#"):
                a, _, b = line.partition("\t")
                out[a] = b.strip()
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("testset")
    ap.add_argument("out")
    ap.add_argument("--report", action="store_true", help="list characters that needed a default")
    args = ap.parse_args()

    engine = Engine(use_user_model=False)
    lex = engine.lexicon
    defaults = load_pairs(HERE / "char_defaults.tsv")
    fixes = load_fixes(HERE / "reading_fixes.tsv")
    undecided: dict[str, set[str]] = {}

    def choose(word: str, forced: dict[str, str]) -> list[str]:
        options = lex.readings(word)
        if forced:
            hit = [syls for syls, _ in options
                   if all(syls[i] == forced[ch] for i, ch in enumerate(word) if ch in forced)]
            if hit:
                return list(hit[0])
        plain = [(s, t) for s, t in options if not t.endswith(":var")] or options
        if len(word) == 1 and len({s for s, _ in plain}) > 1:
            if word in defaults:
                return [defaults[word]]
            undecided.setdefault(word, set()).update(s[0] for s, _ in plain)
        # Prefer the reading in which most characters take their everyday reading,
        # then dictionary readings over ":alt" spellings, then the source priority.
        def rank(st):
            syls, tag = st
            agree = sum(1 for ch, s in zip(word, syls) if defaults.get(ch) == s)
            neutral = sum(1 for ch, s in zip(word, syls) if s.startswith("˙") and ch not in KEEP_NEUTRAL)
            return (-agree, neutral, PRIORITY.get(tag.split(":")[0], 9))

        plain.sort(key=rank)
        return list(plain[0][0])

    with open(args.testset, encoding="utf-8", newline="") as fh, open(args.out, "w", encoding="utf-8") as out:
        out.write("id\tbopomofo\n")
        for row in csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE):
            forced = dict(_RE_FORCE.findall(row.get("note") or ""))
            syls: list[str] = []
            for word in segment(row["text"], lex, engine.lm):
                syls.extend(choose(word, {c: r for c, r in forced.items() if c in word}))
            for rid, sub, fixed in fixes:
                if rid in ("*", row["id"]) and len(fixed) == len(sub):
                    at = row["text"].find(sub)
                    while at != -1:
                        syls[at:at + len(sub)] = fixed
                        at = row["text"].find(sub, at + 1)
            if len(syls) != len(row["text"]):
                print(f"length mismatch: {row['id']} {row['text']}", file=sys.stderr)
            out.write(f"{row['id']}\t{' '.join(syls)}\n")
    if args.report:
        for ch, opts in sorted(undecided.items()):
            print(f"{ch}\t{'/'.join(sorted(opts))}")


if __name__ == "__main__":
    main()
