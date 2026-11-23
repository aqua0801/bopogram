"""Record reference transcripts of the engine protocol, for checking another engine.

Usage (WSL):
    uv run --project ../prototype python tools/make_golden.py RUNTIME_CONFIG OUT_DIR [--dev N]

It starts `bopogram serve` (the Python prototype) once per run, sends a fixed
series of requests, and writes every request with the response it got to
OUT_DIR/golden.jsonl. A port of the engine (the Rust engine) passes when it
gives the same responses; see prototype/docs/golden.md.

The responses contain candidate lists, which come from the dictionary, so the
output stays next to the runtime bundle and is never committed.
"""

from __future__ import annotations

import argparse
import csv
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "prototype"))
from bopogram.layouts.dachen import DACHEN  # noqa: E402


def load(testset: Path, readings: Path, limit: int | None = None) -> list[tuple[str, str, list[str]]]:
    with open(readings, encoding="utf-8") as fh:
        rd = {p[0]: p[1].split() for p in (l.rstrip("\n").split("\t") for l in fh) if len(p) >= 2 and p[0] != "id"}
    with open(testset, encoding="utf-8", newline="") as fh:
        rows = [r for r in csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE) if r["id"] in rd]
    rows = rows[:limit] if limit else rows
    return [(r["id"], r.get("context") or "", rd[r["id"]]) for r in rows]


def keys(syllables: list[str]) -> list[dict]:
    return [{"op": "key", "key": k} for k in DACHEN.to_keys(syllables)]


def typing(syllables_text: str) -> list[dict]:
    return keys(syllables_text.split())


# -- runs -------------------------------------------------------------------------
def run_sentences(sentences, candidates: bool) -> list[dict]:
    """Every sentence typed key by key with learning off; candidates at both ends."""
    reqs = [{"op": "learning", "enabled": False}, {"op": "style", "style": "formal"}]
    for _sid, ctx, syls in sentences:
        reqs.append({"op": "context", "text": ctx})
        reqs += keys(syls)
        if candidates:
            reqs += [{"op": "candidates"}, {"op": "home"}, {"op": "candidates"}, {"op": "end"}]
        reqs.append({"op": "commit"})
    return reqs


def run_editing() -> list[dict]:
    """Editing rules: cursor moves, deletes, picks, symbols, styles, sessions."""
    r: list[dict] = [{"op": "learning", "enabled": False}]
    # 我在家 / 我再看看 with edits in the middle.
    r += typing("ㄨㄛˇ ㄗㄞˋ ㄐㄧㄚ")
    r += [{"op": "left"}, {"op": "left"}, {"op": "candidates"}, {"op": "select", "index": 1},
          {"op": "state"}, {"op": "left"}, {"op": "backspace"}, {"op": "delete"}, {"op": "end"}]
    r += typing("ㄎㄢˋ ㄎㄢˋ")
    r += [{"op": "cycle", "delta": 1}, {"op": "cycle", "delta": -1}, {"op": "commit"}]
    # Pending syllable, backspace inside it, escape.
    r += [{"op": "key", "key": "j"}, {"op": "key", "key": "i"}, {"op": "backspace"}, {"op": "escape"},
          {"op": "escape"}]
    # Space as first tone and as a space; symbols and text; alternatives of a symbol.
    r += typing("ㄅㄛ ㄅㄛ") + [{"op": "key", "key": " "}, {"op": "symbol", "key": ","},
                              {"op": "text", "text": "AB1"}, {"op": "left"}, {"op": "left"},
                              {"op": "left"}, {"op": "left"}, {"op": "candidates"},
                              {"op": "select", "index": 1}, {"op": "commit"}]
    # Styles: the particle.
    for style in ("casual", "formal"):
        r += [{"op": "style", "style": style}] + typing("ㄆㄠˇ ˙ㄉㄜ ㄏㄣˇ ㄎㄨㄞˋ") + [{"op": "commit"}]
    r += [{"op": "style", "style": "bogus"}, {"op": "style", "style": "casual"}]
    # Tone sandhi of 一 / 不.
    r += typing("ㄧˊ ㄍㄜˋ ㄖㄣˊ") + [{"op": "commit"}]
    r += typing("ㄅㄨˊ ㄧㄠˋ") + [{"op": "commit"}]
    r += typing("ㄧˋ") + [{"op": "state"}] + typing("ㄑㄧˇ") + [{"op": "commit"}]
    # A long buffer commits its first words on its own.
    r += typing(" ".join(["ㄨㄛˇ ㄇㄣˊ ㄧㄠˋ ㄑㄩˋ ㄎㄢˋ ㄉㄧㄢˋ ㄧㄥˇ"] * 6)) + [{"op": "commit"}]
    # Context, sessions, unknown requests, keys outside the layout.
    r += [{"op": "context", "text": "今天天氣很好，"}] + typing("ㄨㄛˇ ㄇㄣˊ ㄑㄩˋ ㄍㄨㄥ ㄩㄢˊ") + [{"op": "commit"}]
    r += [{"op": "key", "key": "j", "sid": 7}, {"op": "key", "key": "3", "sid": 7}, {"op": "state", "sid": 7},
          {"op": "state"}, {"op": "close", "sid": 7}, {"op": "state", "sid": 7}]
    r += [{"op": "nonsense"}, {"op": "key", "key": "!"}, {"op": "commit"}]
    return r


def run_userdict() -> list[dict]:
    """The user's dictionary, starting from the files written by setup()."""
    r: list[dict] = [{"op": "dict_list"}]
    for text in ("長大", "波波", "視頻", "長" * 9, "長長長"):
        r.append({"op": "readings", "text": text})
    # Adding words; the change shows on the next key.
    r += typing("ㄅㄛ ㄅㄛ ㄏㄠˇ")
    r += [{"op": "dict_add", "word": "伯伯", "reading": "ㄅㄛ ㄅㄛ"}, {"op": "state"}]
    r += typing("ㄇㄚ") + [{"op": "candidates"}, {"op": "commit"}]
    r += typing("ㄕˋ ㄆㄧㄣˊ ㄏㄣˇ ㄏㄠˇ") + [{"op": "commit"}]
    r += typing("ㄔㄤˊ ㄉㄚˋ") + [{"op": "home"}, {"op": "candidates"}]
    r += [{"op": "dict_add", "start": 0, "length": 2}, {"op": "commit"}]
    # Removing, excluding, restoring.
    r += [{"op": "dict_remove", "word": "伯伯"}, {"op": "dict_remove", "word": "視頻"}]
    r += typing("ㄕˋ ㄆㄧㄣˊ ㄏㄣˇ ㄏㄠˇ") + [{"op": "candidates"}, {"op": "commit"}]
    r += [{"op": "dict_exclude", "text": "頻道"}]
    r += typing("ㄉㄧㄢˋ ㄕˋ ㄆㄧㄣˊ ㄉㄠˋ") + [{"op": "commit"}]
    r += typing("ㄒㄧㄣ ㄨㄣˊ ㄆㄧㄣˊ ㄉㄠˋ") + [{"op": "home"}, {"op": "right"}, {"op": "right"},
                                           {"op": "candidates"}]
    r += [{"op": "dict_remove", "start": 2, "length": 2, "word": "沒有這個"},
          {"op": "dict_remove", "start": 2, "length": 1, "word": "頻"}, {"op": "commit"}]
    r += [{"op": "dict_restore", "word": "視頻"}, {"op": "dict_restore", "word": "沒有"}]
    r += typing("ㄕˋ ㄆㄧㄣˊ") + [{"op": "commit"}]
    # Hand-picked text stays even when excluded.
    r += [{"op": "dict_exclude", "text": "視頻"}] + typing("ㄕˋ ㄆㄧㄣˊ")
    r += [{"op": "home"}, {"op": "candidates"}]
    # Errors.
    r += [{"op": "dict_add", "word": "波波", "reading": "ㄅㄛ ㄅ"},
          {"op": "dict_add", "word": "波波", "reading": "ㄅㄛ"},
          {"op": "dict_add", "word": "波" * 9, "reading": " ".join(["ㄅㄛ"] * 9)},
          {"op": "dict_add", "word": "長大", "reading": "ㄔㄤˊ ㄉㄚˋ"},
          {"op": "dict_remove", "word": "沒沒沒"},
          {"op": "dict_exclude", "text": "我"}, {"op": "escape"}, {"op": "dict_list"}]
    return r


def run_learning() -> list[dict]:
    """Personal learning: a word picked by hand wins the next time."""
    r: list[dict] = []
    for _ in range(2):
        r += typing("ㄊㄚ ㄕˋ ㄨㄛˇ ˙ㄉㄜ ㄆㄥˊ ㄧㄡˇ") + [{"op": "home"}, {"op": "candidates"}]
        r += [{"op": "select", "index": 2}, {"op": "commit"}]
    r += typing("ㄊㄚ ㄕˋ ㄨㄛˇ ˙ㄉㄜ ㄆㄥˊ ㄧㄡˇ") + [{"op": "commit"}]
    r += [{"op": "learning", "enabled": False}] + typing("ㄇㄧˋ ㄇㄚˇ ㄕˋ") + [{"op": "commit"}]
    return r


USERDICT = "# set up for the golden run\n波波\tㄅㄛ ㄅㄛ\tuser\n"
EXCLUDED = "質量\n"


# -- recording ----------------------------------------------------------------------
def record(name: str, reqs: list[dict], config: dict, base: Path, out, setup: dict) -> int:
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        files = {"userdict.tsv": setup.get("userdict", ""), "excluded.tsv": setup.get("excluded", "")}
        for fname, text in files.items():
            if text:
                (tmp / fname).write_text(text, encoding="utf-8")
        cfg = dict(config, user_model=str(tmp / "user.json"), typing_log=None,
                   user_dict=str(tmp / "userdict.tsv"), excluded_words=str(tmp / "excluded.tsv"))
        cfg_path = base / f".golden-{name}.json"  # model paths stay relative to the bundle
        cfg_path.write_text(json.dumps(cfg, ensure_ascii=False), encoding="utf-8")
        try:
            proc = subprocess.Popen([sys.executable, "-m", "bopogram.cli", "--config", str(cfg_path), "serve"],
                                    cwd=ROOT / "prototype", stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=subprocess.DEVNULL, text=True, encoding="utf-8")
            ready = json.loads(proc.stdout.readline())
            out.write(json.dumps({"run": name, "setup": {"config": {k: v for k, v in config.items()
                                                                    if k not in ("dict", "models")},
                                                         "files": files}, "ready": ready},
                                 ensure_ascii=False) + "\n")
            for n, req in enumerate(reqs):
                req = dict(req, id=n)
                proc.stdin.write(json.dumps(req, ensure_ascii=False) + "\n")
                proc.stdin.flush()
                res = json.loads(proc.stdout.readline())
                out.write(json.dumps({"run": name, "req": req, "res": res}, ensure_ascii=False) + "\n")
            proc.stdin.write('{"op": "quit"}\n')
            proc.stdin.close()
            proc.wait(timeout=60)
            end = {"user.json": json.loads((tmp / "user.json").read_text(encoding="utf-8"))
                   if (tmp / "user.json").exists() else None}
            for fname in files:
                p = tmp / fname
                end[fname] = p.read_text(encoding="utf-8") if p.exists() else None
            out.write(json.dumps({"run": name, "end": end}, ensure_ascii=False) + "\n")
        finally:
            cfg_path.unlink(missing_ok=True)
    return len(reqs)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("config", type=Path, help="config.json of the runtime bundle")
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--dev", type=int, default=1000, help="development sentences to include")
    args = ap.parse_args()
    config = json.loads(args.config.read_text(encoding="utf-8"))
    base = args.config.resolve().parent
    test = load(ROOT / "eval/testset/testset.tsv", ROOT / "eval/readings/testset_bopomofo.tsv")
    dev = load(ROOT / "eval/devset/moztw_dev.tsv", ROOT / "eval/readings/moztw_dev_bopomofo.tsv", args.dev)
    runs = [("testset", run_sentences(test, True), {}),
            ("devset", run_sentences(dev, False), {}),
            ("editing", run_editing(), {}),
            ("userdict", run_userdict(), {"userdict": USERDICT, "excluded": EXCLUDED}),
            ("learning", run_learning(), {})]
    args.out_dir.mkdir(parents=True, exist_ok=True)
    tmp = args.out_dir / "golden.jsonl.tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as out:
        for name, reqs, setup in runs:
            n = record(name, reqs, config, base, out, setup)
            print(f"{name}: {n} requests", flush=True)
    tmp.replace(args.out_dir / "golden.jsonl")
    shutil.copy(__file__, args.out_dir / "make_golden.py")  # how it was made, next to the data


if __name__ == "__main__":
    main()
