"""Why is the right sentence not among the n-gram decoder's results?

Usage (from prototype/):
    uv run python ../eval/tools/analyze_misses.py CONFIG TESTSET READINGS OUT_TSV [--limit N]

Decodes every sentence as the engine does when the character model re-ranks
(no merging of equal model states, beam 16) and finds where the right sentence
is among the 20 best results. Differences the evaluation accepts (the other
standard form of a character, and 他/她/它) do not count. For a sentence whose
right answer is not among the 20, the first step that loses it decides its class:

  missing     a character of it is not a candidate for its syllable at all:
              the dictionary lacks that reading, or the sentence's reading is wrong
  filtered    every character is in the dictionary, but the decoder's candidate
              limits (max_candidates per syllable, the rare-word filter) drop one
  search      it appears when decoded with beam 256 and no candidate limits:
              lost to the beam
  scoring     it is reachable but scores lower: the n-gram model prefers another.
              Also reported: the score gap, and whether the character model
              prefers the right sentence over the decoder's best

OUT_TSV lists every sentence not right at first, with its class and details.
"""

from __future__ import annotations

import argparse
import csv
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "prototype"))
from bopogram.context import context_state  # noqa: E402
from bopogram.decoder import is_literal  # noqa: E402
from bopogram.engine import Engine  # noqa: E402
from bopogram.evaluate import _PRONOUNS, accept_variants, load_readings, load_testset  # noqa: E402

TOP = 20


def same(gold: str, hyp: str) -> bool:
    return accept_variants(gold, hyp).translate(_PRONOUNS) == hyp.translate(_PRONOUNS)


def decode(engine: Engine, syls: list[str], context: str, beam: int = 16, max_candidates: int | None = None,
           pinned: dict | None = None):
    dec = engine.new_decoder()
    dec.merge = False
    dec.beam = beam
    if max_candidates is not None:
        dec.max_candidates = max_candidates
    state = context_state(context, engine.lexicon, engine.lm)
    dec.reset(state)
    if pinned:
        dec.pinned = dict(pinned)
    for s in syls:
        dec.push(s)
    return dec


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("config", type=Path)
    ap.add_argument("testset")
    ap.add_argument("readings")
    ap.add_argument("out", type=Path)
    ap.add_argument("--limit", type=int)
    args = ap.parse_args()
    engine = Engine(args.config, use_user_model=False)
    engine.typing_log = None
    lex = engine.lexicon
    readings = load_readings(args.readings)
    rows = [r for r in load_testset(args.testset) if r["id"] in readings][: args.limit]
    ranks, classes, prefers = Counter(), Counter(), Counter()
    out = []
    for row in rows:
        syls, gold, ctx = readings[row["id"]], row["text"], row.get("context") or ""
        if len(syls) != len(gold):
            continue
        dec = decode(engine, syls, ctx)
        hyps = [("".join(w), s) for w, s in dec.nbest(TOP)]
        rank = next((i for i, (h, _) in enumerate(hyps) if same(gold, h)), None)
        ranks["1" if rank == 0 else "2-5" if rank is not None and rank < 5 else
              "6-20" if rank is not None else "not in 20"] += 1
        if rank == 0:
            continue
        best, best_score = hyps[0]
        cls, detail = "", ""
        if rank is None:
            missing = [(i, g, s) for i, (g, s) in enumerate(zip(gold, syls))
                       if not is_literal(s) and g not in lex.candidates((s,))]
            if missing:
                cls = "missing"
                detail = " ".join(f"{g}:{s}(dict:{'/'.join(r[0] for r, _ in lex.readings(g) if len(r) == 1) or '-'})"
                                  for _, g, s in missing)
            else:
                cut = [(g, s) for g, s in zip(gold, syls) if not is_literal(s) and g not in dec._candidates((s,))]
                if cut:
                    cls, detail = "filtered", " ".join(f"{g}:{s}" for g, s in cut)
                else:
                    wide = decode(engine, syls, ctx, beam=256, max_candidates=100_000)
                    if any(same(gold, "".join(w)) for w, _ in wide.nbest(50)):
                        cls = "search"
                    else:
                        cls = "scoring"
                        pins = {(i, i + 1): g for i, (g, s) in enumerate(zip(gold, syls)) if not is_literal(s)}
                        forced = decode(engine, syls, ctx, beam=256, max_candidates=100_000, pinned=pins)
                        res = forced.nbest(1)
                        gap = best_score - res[0][1] if res else float("nan")
                        detail = f"gap={gap:.2f}"
                        if engine.charlm is not None:
                            a, b = engine.charlm.score(ctx, [best, gold])
                            pref = "charlm prefers right" if b > a else "charlm prefers wrong"
                            prefers[pref] += 1
                            detail += f" {pref} ({b - a:+.2f})"
            classes[cls] += 1
        out.append((row["id"], rank + 1 if rank is not None else "", cls, gold, best, detail))
    n = sum(ranks.values())
    print(f"{n} sentences; right answer at rank: " +
          ", ".join(f"{k} {ranks[k]} ({ranks[k] / n:.1%})" for k in ("1", "2-5", "6-20", "not in 20")))
    total = sum(classes.values())
    print("not in 20 by class: " + ", ".join(f"{k} {v} ({v / total:.0%})" for k, v in classes.most_common()))
    if prefers:
        print("scoring class: " + ", ".join(f"{k} {v}" for k, v in prefers.most_common()))
    with open(args.out, "w", encoding="utf-8", newline="") as fh:
        w = csv.writer(fh, delimiter="\t", lineterminator="\n")
        w.writerow(["id", "rank", "class", "gold", "best", "detail"])
        w.writerows(out)


if __name__ == "__main__":
    main()
