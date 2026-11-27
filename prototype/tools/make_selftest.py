"""Expected results for a model pack's self-test (prototype/docs/release-spec.md).

Usage (WSL, from prototype/):
    uv run python tools/make_selftest.py CONFIG TESTSET READINGS OUT_JSON [--n 20]

Converts the first N test sentences with the engine and the models of CONFIG,
in both styles, without learning, and writes what the engine chose:

    {"format": 1, "cases": [{"bopomofo": "ㄨㄛˇ ㄗㄞˋ ...", "context": "", "style": "formal",
                             "expect": "我再看看好了"}, ...]}

After installing or updating a model pack, the program converts the same
input and must get exactly `expect` (the expected text, not the right answer:
it checks that the models were installed and load correctly).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from bopogram.engine import Engine  # noqa: E402
from bopogram.evaluate import load_readings, load_testset  # noqa: E402


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("config", type=Path)
    ap.add_argument("testset")
    ap.add_argument("readings")
    ap.add_argument("out", type=Path)
    ap.add_argument("--n", type=int, default=20)
    args = ap.parse_args()
    engine = Engine(args.config, use_user_model=False)
    engine.typing_log = None
    readings = load_readings(args.readings)
    rows = [r for r in load_testset(args.testset) if r["id"] in readings][: args.n]
    cases = []
    for style in ("formal", "casual"):
        engine.set_style(style)
        for r in rows:
            syls = readings[r["id"]]
            text = engine.convert(syls, r.get("context") or "")[0][0]
            cases.append({"bopomofo": " ".join(syls), "context": r.get("context") or "", "style": style,
                          "expect": text})
    args.out.write_text(json.dumps({"format": 1, "cases": cases}, ensure_ascii=False, indent=1) + "\n",
                        encoding="utf-8")
    print(f"{len(cases)} cases written to {args.out}")


if __name__ == "__main__":
    main()
