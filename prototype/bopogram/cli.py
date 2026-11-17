"""Command-line interface. The Rust engine must keep the same commands.

    bopogram convert --keys "ji3y94ru8 "
    bopogram convert --bopomofo "ㄨㄛˇ ㄗㄞˋ ㄐㄧㄚ" --context "上文" --nbest 5
    bopogram eval eval/testset/testset.tsv --readings eval/readings/testset_bopomofo.tsv
    bopogram serve          # JSON lines on stdin/stdout, see bopogram/server.py
    bopogram lookup 垃圾 行   # how the dictionary reads these words / characters
    bopogram lookup --bopomofo "ㄌㄜˋ ㄙㄜˋ"   # what a reading can become
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
    c.add_argument("--style", choices=["casual", "formal"], help="override the configured style")

    lk = sub.add_parser("lookup", help="show dictionary readings of words, or the words of a reading")
    lk.add_argument("words", nargs="*", help="words or single characters")
    lk.add_argument("--bopomofo", help="syllables separated by spaces: list the words read this way")
    lk.add_argument("--layout", default="dachen", choices=sorted(LAYOUTS))

    s = sub.add_parser("serve", help="JSON-lines protocol on stdin/stdout for a front end")
    s.add_argument("--layout", default="dachen", choices=sorted(LAYOUTS))
    s.add_argument("--trace", help="debugging: append every request and response to this file "
                                   "(records everything typed; do not leave it on)")

    e = sub.add_parser("eval", help="score a test set")
    e.add_argument("testset")
    e.add_argument("--readings", required=True, help="TSV: id, bopomofo syllables")
    e.add_argument("--system", default="bopogram", choices=["bopogram", "word-unigram", "char-unigram"])
    e.add_argument("--char-freq", help="character frequency TSV (char-unigram baseline)")
    e.add_argument("--style", default="formal", choices=["casual", "formal"],
                   help="the test set uses standard 的/地/得, so it is scored in formal style by default")
    e.add_argument("--neural-model", help="ONNX model to run live after every syllable "
                                          "(overrides the config; latency then includes the model)")
    e.add_argument("--neural-scores", help="JSONL from training/neural/score_testset.py")
    e.add_argument("--neural-mode", default="fuse", choices=["fuse", "rerank", "alone"],
                   help="fuse: add the scores inside the lattice; rerank: re-order the n-gram "
                        "engine's 10 best results; alone: the neural model's own choice")
    e.add_argument("--neural-weight", type=float, default=1.0)
    e.add_argument("--neural-floor", type=float, default=-12.0)
    e.add_argument("--errors", help="write wrong sentences to this file")
    e.add_argument("--dump-nbest", help="write the engine's best results per sentence (JSONL) for external rescoring")
    e.add_argument("--nbest", type=int, default=10)
    e.add_argument("--beam", type=int, help="with --dump-nbest: beam width, without merging equal model states")
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
        if args.style:
            engine.set_style(args.style)
        for text, score in engine.convert(syllables, context=args.context, nbest=args.nbest):
            print(f"{text}\t{score:.3f}" if args.nbest > 1 else text)
        return 0

    if args.cmd == "lookup":
        import json

        from .engine import config_path
        from .lexicon import Lexicon

        path = config_path(args.config)
        lexicon = Lexicon((path.parent / json.loads(path.read_text(encoding="utf-8"))["dict"]).resolve())
        layout = LAYOUTS[args.layout]
        if args.bopomofo:
            words = lexicon.candidates(tuple(args.bopomofo.split()))
            print(" ".join(words) if words else "(the dictionary has no word with this reading)")

        def show(word: str, indent: str = "") -> bool:
            readings = lexicon.readings(word)
            for syls, tag in readings:
                note = {"var": "  (variant reading)", "alt": "  (citation-tone spelling)"}.get(tag.partition(":")[2], "")
                print(f"{indent}{word}\t{' '.join(syls)}\tkeys: {layout.to_keys(list(syls))!r}{note}")
            return bool(readings)

        for word in args.words:
            if not show(word):
                print(f"{word}\t(not in the dictionary as a word)")
            if len(word) > 1:  # also show each character, to see which one is the problem
                for ch in dict.fromkeys(word):
                    if not show(ch, "  "):
                        print(f"  {ch}\t(not in the dictionary: this character cannot be typed)")
        return 0

    if args.cmd == "serve":
        from .server import serve

        return serve(args.config, args.layout, args.trace)

    from .evaluate import run_eval

    return run_eval(args)


if __name__ == "__main__":
    sys.exit(main())
