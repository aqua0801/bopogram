"""Estimate mixture weights for the language models on the development set (EM).

Usage (from prototype/):
    uv run python ../eval/tools/tune_weights.py DEV_TSV [--iterations 30] [--write]

Each development sentence is segmented into dictionary words; every model
scores every word in context; EM then finds the linear-interpolation weights
that maximise the likelihood of the development text. Only the development
set is used, never the test set. --write stores the weights in the engine
config.
"""

from __future__ import annotations

import argparse
import csv
import json
import math

import kenlm

from bopogram.context import segment
from bopogram.engine import Engine, config_path


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("dev_tsv")
    ap.add_argument("--iterations", type=int, default=30)
    ap.add_argument("--write", action="store_true")
    args = ap.parse_args()

    engine = Engine(use_user_model=False)
    lm, lex = engine.lm, engine.lexicon
    probs: list[list[float]] = []  # per token: probability under each model
    with open(args.dev_tsv, encoding="utf-8", newline="") as fh:
        for row in csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE):
            tokens = [lm.map_token(t, lex.__contains__) for t in segment(row["text"], lex, lm)]
            states = []
            for m in lm.models:
                s = kenlm.State()
                m.BeginSentenceWrite(s)
                states.append(s)
            for tok in tokens:
                row_p = []
                for k, m in enumerate(lm.models):
                    out = kenlm.State()
                    row_p.append(10.0 ** m.BaseScore(states[k], tok, out))
                    states[k] = out
                probs.append(row_p)

    n = len(lm.models)
    w = [1.0 / n] * n
    for it in range(args.iterations):
        resp = [0.0] * n
        ll = 0.0
        for row_p in probs:
            mix = sum(wi * p for wi, p in zip(w, row_p))
            ll += math.log(mix)
            for k in range(n):
                resp[k] += w[k] * row_p[k] / mix
        w = [r / len(probs) for r in resp]
    ppl = math.exp(-ll / len(probs))
    path = config_path()
    cfg = json.loads(path.read_text(encoding="utf-8"))
    for m, wi in zip(cfg["models"], w):
        print(f"{m['path']:<20} {wi:.4f}")
    print(f"tokens {len(probs)}, dev perplexity {ppl:.1f}")
    for k, m in enumerate(cfg["models"]):
        single = math.exp(-sum(math.log(p[k]) for p in probs) / len(probs))
        print(f"  {m['path']} alone: perplexity {single:.1f}")
    if args.write:
        for m, wi in zip(cfg["models"], w):
            m["weight"] = round(wi, 4)
        path.write_text(json.dumps(cfg, ensure_ascii=False, indent=1), encoding="utf-8")
        print("weights written to", path)


if __name__ == "__main__":
    main()
