"""Record what the re-ranker sees, so ways of re-ranking can be compared offline.

Usage (from prototype/):
    uv run python ../eval/tools/dump_rerank.py CONFIG TESTSET READINGS OUT_JSONL \
        --charlm standard=PATH --charlm advanced=PATH [--top 5] [--limit N]

For every sentence, decoded as the engine does before re-ranking (formal style,
no merging of equal model states), writes one line:

    {"id", "gold", "context",
     "hyps": [{"text", "ngram"}, ...],          # nbest(top), in the decoder's order
     "charlm": {"standard": [...], ...}}        # ln P of each text under each model

`ngram` is the decoder's path score (the engine adds weight x charlm to it and
takes the highest; the order also includes the end-of-sentence term). Resumes
when OUT_JSONL exists: sentences already written are skipped.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "prototype"))
from bopogram.charlm import CharLMScorer  # noqa: E402
from bopogram.context import context_state  # noqa: E402
from bopogram.engine import Engine  # noqa: E402
from bopogram.evaluate import load_readings, load_testset  # noqa: E402


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("config", type=Path)
    ap.add_argument("testset")
    ap.add_argument("readings")
    ap.add_argument("out", type=Path)
    ap.add_argument("--charlm", action="append", default=[], help="NAME=PATH of a model in the engine's format")
    ap.add_argument("--top", type=int, default=5)
    ap.add_argument("--limit", type=int)
    args = ap.parse_args()
    engine = Engine(args.config, use_user_model=False)
    engine.typing_log = None
    models = {}
    for spec in args.charlm:
        name, path = spec.split("=", 1)
        models[name] = CharLMScorer(Path(path))
    readings = load_readings(args.readings)
    rows = [r for r in load_testset(args.testset) if r["id"] in readings][: args.limit]
    done = set()
    if args.out.exists():
        with open(args.out, encoding="utf-8") as fh:
            done = {json.loads(line)["id"] for line in fh}
    with open(args.out, "a", encoding="utf-8") as out:
        for n, row in enumerate(rows):
            if row["id"] in done:
                continue
            syls, gold, ctx = readings[row["id"]], row["text"], row.get("context") or ""
            if len(syls) != len(gold):
                continue
            dec = engine.new_decoder()
            dec.merge = False
            dec.style = "formal"
            dec.reset(context_state(ctx, engine.lexicon, engine.lm))
            for s in syls:
                dec.push(s)
            hyps = [{"text": "".join(w), "ngram": round(sc, 5)} for w, sc in dec.nbest(args.top)]
            texts = [h["text"] for h in hyps]
            scores = {name: [round(v, 5) for v in m.score(ctx, texts)] for name, m in models.items()}
            out.write(json.dumps({"id": row["id"], "gold": gold, "context": ctx, "hyps": hyps, "charlm": scores},
                                 ensure_ascii=False) + "\n")
            if n % 500 == 0:
                out.flush()
                print(f"{n} / {len(rows)}", flush=True)


if __name__ == "__main__":
    main()
