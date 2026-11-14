"""Grid-search decoder settings on the development set.

Usage (from prototype/):
    uv run python ../eval/tools/tune_decoder.py DEV_TSV DEV_READINGS [--limit 4000]

Tries combinations of the per-word penalty and the end-of-sentence weight
and prints sentence / character accuracy for each. Uses only the development
set. Nothing is written; put the chosen values into the engine config.
"""

from __future__ import annotations

import argparse
import itertools

from bopogram.engine import Engine
from bopogram.evaluate import load_readings, load_testset


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("dev_tsv")
    ap.add_argument("readings")
    ap.add_argument("--limit", type=int, default=4000)
    ap.add_argument("--word-penalty", type=float, nargs="*", default=[-1.0, -0.5, 0.0, 0.5, 1.0])
    ap.add_argument("--eos-weight", type=float, nargs="*", default=[0.0, 0.5, 1.0])
    ap.add_argument("--beam", type=int, nargs="*", default=[16])
    args = ap.parse_args()

    engine = Engine(use_user_model=False)
    dec = engine.decoder
    readings = load_readings(args.readings)
    rows = [r for r in load_testset(args.dev_tsv) if len(readings.get(r["id"], [])) == len(r["text"])]
    rows = rows[: args.limit]
    print(f"{len(rows)} sentences")
    for beam, wp, ew in itertools.product(args.beam, args.word_penalty, args.eos_weight):
        dec.beam, dec.word_penalty, dec.eos_weight = beam, wp, ew
        ok = chars = chars_ok = 0
        for r in rows:
            hyp = "".join(dec.decode(readings[r["id"]])[0][0])
            ok += hyp == r["text"]
            chars += len(r["text"])
            chars_ok += sum(a == b for a, b in zip(hyp, r["text"]))
        print(f"beam {beam:>3}  word_penalty {wp:>5.2f}  eos_weight {ew:>4.2f}  "
              f"sentence {ok / len(rows):.2%}  char {chars_ok / chars:.2%}", flush=True)


if __name__ == "__main__":
    main()
