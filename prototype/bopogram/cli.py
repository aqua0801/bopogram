"""Command-line interface. The Rust engine must keep the same commands.

    bopogram convert --keys "ji3y94ru8 "
    bopogram convert --bopomofo "ㄨㄛˇ ㄗㄞˋ ㄐㄧㄚ" --context "上文" --nbest 5
    bopogram eval eval/testset/testset.tsv --readings eval/readings/testset_bopomofo.tsv
    bopogram serve          # JSON lines on stdin/stdout, see bopogram/server.py
"""

from __future__ import annotations

import argparse
import sys

from .layouts import LAYOUTS, LayoutError


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="bopogram", description="Context-aware bopomofo conversion")
    ap.add_argument("--config", help="engine config JSON (default: $BOPOGRAM_CONFIG)")
    sub = ap.add_subparsers(dest="cmd", required=True)

    c = sub.add_parser("convert", help="convert keystrokes or bopomofo to text")
    src = c.add_mutually_exclusive_group(required=True)
    src.add_argument("--keys", help="raw keystrokes in the chosen layout")
    src.add_argument("--bopomofo", help="syllables separated by spaces")
    c.add_argument("--layout", default="dachen", choices=sorted(LAYOUTS))
    c.add_argument("--context", default="", help="text before the cursor (read-only)")
    c.add_argument("--nbest", type=int, default=1)

    s = sub.add_parser("serve", help="JSON-lines protocol on stdin/stdout for a front end")
    s.add_argument("--layout", default="dachen", choices=sorted(LAYOUTS))

    e = sub.add_parser("eval", help="score a test set")
    e.add_argument("testset")
    e.add_argument("--readings", required=True, help="TSV: id, bopomofo syllables")
    e.add_argument("--system", default="bopogram", choices=["bopogram", "word-unigram", "char-unigram"])
    e.add_argument("--char-freq", help="character frequency TSV (char-unigram baseline)")
    e.add_argument("--errors", help="write wrong sentences to this file")
    e.add_argument("--json", help="write the metrics to this file")
    args = ap.parse_args(argv)

    if args.cmd == "convert":
        from .engine import Engine

        try:
            syllables = (LAYOUTS[args.layout].to_syllables(args.keys) if args.keys is not None
                         else args.bopomofo.split())
        except LayoutError as err:
            print(f"error: {err}", file=sys.stderr)
            return 2
        engine = Engine(args.config, use_user_model=False)
        for text, score in engine.convert(syllables, context=args.context, nbest=args.nbest):
            print(f"{text}\t{score:.3f}" if args.nbest > 1 else text)
        return 0

    if args.cmd == "serve":
        from .server import serve

        return serve(args.config, args.layout)

    from .evaluate import run_eval

    return run_eval(args)


if __name__ == "__main__":
    sys.exit(main())
