"""Simulate a user who types a stream of sentences and corrects every mistake,
to measure what personal learning gains (and whether it learns too much).

Usage (WSL):
    uv run --project ../prototype python tools/simulate_learning.py CONFIG TESTSET READINGS
        [--limit N] [--passes 2] [--variant NAME ...]

For each sentence: type its syllables, compare with the reference, and while it
differs move the cursor to the first wrong character and pick the longest
candidate that matches the reference there (as a user would), then commit, so
the engine learns from the corrected text. Reported per pass: sentences right
before any correction, and picks needed per 100 sentences. The first pass shows
how learning carries over to new sentences; later passes show how well it
remembers corrections. Variants change the learning settings (see VARIANTS).
"""

from __future__ import annotations

import argparse
import csv
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "prototype"))
from bopogram import user as user_mod  # noqa: E402
from bopogram.engine import Engine  # noqa: E402
from bopogram.session import Session  # noqa: E402

# name -> settings for UserModel (None = learning off)
VARIANTS: dict[str, dict | None] = {
    "off": None,
    "current": {},  # the defaults: corrections only, weight 2
    "all": {"accepted_step": 1, "weight": 1.0},  # the previous defaults: learn all committed text
    "w1": {"weight": 1.0},
    "w3": {"weight": 3.0},
    "uni1": {"unigram_weight": 1.0},
    "step6": {"correction_step": 6},
}


def load(testset: Path, readings: Path, limit: int | None):
    with open(readings, encoding="utf-8") as fh:
        rd = {p[0]: p[1].split() for p in (l.rstrip("\n").split("\t") for l in fh) if len(p) >= 2 and p[0] != "id"}
    with open(testset, encoding="utf-8", newline="") as fh:
        rows = [r for r in csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE) if r["id"] in rd]
    rows = [(r.get("context") or "", rd[r["id"]], r["text"]) for r in rows if len(r["text"]) == len(rd[r["id"]])]
    return rows[:limit] if limit else rows


def type_sentence(s: Session, ctx: str, syls: list[str], gold: str) -> tuple[bool, int]:
    """(right at first, picks needed); the sentence is committed afterwards."""
    s.set_context(ctx)
    for syl in syls:
        s._insert(syl)
    first = picks = 0
    for _ in range(len(gold) + 1):
        items = [t for t, _, _ in s._items()]
        wrong = next((k for k, (t, g) in enumerate(zip(items, gold)) if t != g), None)
        if wrong is None:
            break
        if picks == 0:
            first = 1
        s.cursor = wrong
        cands = s.candidates()
        pick = max((i for i, c in enumerate(cands) if gold[wrong:wrong + c["length"]] == c["word"]),
                   key=lambda i: cands[i]["length"], default=None)
        if pick is None:
            break  # the right word cannot be typed this way; give up on the sentence
        s.select(pick)
        picks += 1
    s.commit()
    return not first, picks


def run(engine: Engine, rows, settings: dict | None, passes: int) -> list[tuple[float, float]]:
    with tempfile.TemporaryDirectory() as tmp:
        engine.user = None if settings is None else user_mod.UserModel(Path(tmp) / "user.json", **settings)
        s = Session(engine)
        s.set_style("formal")
        out = []
        for _ in range(passes):
            right = picks = 0
            for ctx, syls, gold in rows:
                ok, n = type_sentence(s, ctx, syls, gold)
                right += ok
                picks += n
            out.append((right / len(rows), 100 * picks / len(rows)))
        engine.user = None
        return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("config", type=Path)
    ap.add_argument("testset", type=Path)
    ap.add_argument("readings", type=Path)
    ap.add_argument("--limit", type=int)
    ap.add_argument("--passes", type=int, default=2)
    ap.add_argument("--variant", nargs="*", default=list(VARIANTS))
    ap.add_argument("--no-charlm", action="store_true", help="without the character LM re-ranking")
    args = ap.parse_args()
    rows = load(args.testset, args.readings, args.limit)
    engine = Engine(args.config, use_user_model=False)
    engine.typing_log = None
    if args.no_charlm:
        engine.charlm = None
    print(f"{len(rows)} sentences, {args.passes} passes", flush=True)
    for name in args.variant:
        t = time.time()
        res = run(engine, rows, VARIANTS[name], args.passes)
        cols = "  ".join(f"pass {i + 1}: right {r:.1%} picks/100 {p:.1f}" for i, (r, p) in enumerate(res))
        print(f"{name:12} {cols}   ({time.time() - t:.0f} s)", flush=True)


if __name__ == "__main__":
    main()
