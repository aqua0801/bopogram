"""Replay the reference transcripts (make_golden.py) against an engine and compare.

Usage:
    python tools/check_golden.py GOLDEN_JSONL RUNTIME_CONFIG --cmd "ENGINE --config {config} serve"
                                 [--runs testset,editing] [--show 20]

ENGINE is any program that speaks the protocol of prototype/bopogram/server.py
on stdin/stdout, for example the Rust engine's `bopogram` command, or
`python -m bopogram.cli` for the Python prototype. For each run the script
writes a config next to RUNTIME_CONFIG (so model paths resolve the same way)
with the run's user files in a fresh temporary directory, starts the engine,
sends the recorded requests one by one and compares every response.

Comparison: responses must be equal as JSON objects, except that the text of
"error" is not compared (only that it is there). At the end of each run the
user files (user.json as JSON, the .tsv files as text) are compared too.
Plain Python 3, no packages needed. Exit status 1 if anything differs.
"""

from __future__ import annotations

import argparse
import json
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path


def same(a: dict, b: dict) -> bool:
    if ("error" in a) != ("error" in b):
        return False
    strip = lambda d: {k: v for k, v in d.items() if k != "error"}
    return strip(a) == strip(b)


def load_runs(path: Path, only: set[str] | None):
    runs: dict[str, dict] = {}
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            d = json.loads(line)
            name = d["run"]
            if only and name not in only:
                continue
            run = runs.setdefault(name, {"steps": []})
            if "setup" in d:
                run["setup"], run["ready"] = d["setup"], d.get("ready")
            elif "end" in d:
                run["end"] = d["end"]
            else:
                run["steps"].append((d["req"], d["res"]))
    return runs


def check_run(name: str, run: dict, cmd: str, config: dict, base: Path, show: int) -> tuple[int, int]:
    bad = 0
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        files = run["setup"]["files"]
        for fname, text in files.items():
            if text:
                (tmp / fname).write_text(text, encoding="utf-8")
        cfg = dict(config, user_model=str(tmp / "user.json"), typing_log=None,
                   user_dict=str(tmp / "userdict.tsv"), excluded_words=str(tmp / "excluded.tsv"))
        cfg_path = base / f".check-{name}.json"
        cfg_path.write_text(json.dumps(cfg, ensure_ascii=False), encoding="utf-8")
        try:
            argv = [a.replace("{config}", str(cfg_path)) for a in shlex.split(cmd)]
            proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=subprocess.DEVNULL, text=True, encoding="utf-8")
            ready = json.loads(proc.stdout.readline())
            if ready != run.get("ready"):
                bad += 1
                print(f"[{name}] ready line: expected {run.get('ready')} got {ready}")
            for req, want in run["steps"]:
                proc.stdin.write(json.dumps(req, ensure_ascii=False) + "\n")
                proc.stdin.flush()
                got = json.loads(proc.stdout.readline())
                if not same(want, got):
                    bad += 1
                    if bad <= show:
                        print(f"[{name}] request {req['id']}: {json.dumps(req, ensure_ascii=False)}")
                        for k in sorted(set(want) | set(got)):
                            if want.get(k) != got.get(k) and k != "error":
                                print(f"    {k}: expected {json.dumps(want.get(k), ensure_ascii=False)}")
                                print(f"    {' ' * len(k)}       got {json.dumps(got.get(k), ensure_ascii=False)}")
            proc.stdin.write('{"op": "quit"}\n')
            proc.stdin.close()
            proc.wait(timeout=60)
            end = run.get("end", {})
            for fname, want in end.items():
                p = tmp / fname
                got = None
                if p.exists():
                    text = p.read_text(encoding="utf-8")
                    got = json.loads(text) if fname.endswith(".json") else text
                if got != want:
                    bad += 1
                    print(f"[{name}] {fname} at the end differs")
        finally:
            cfg_path.unlink(missing_ok=True)
    return len(run["steps"]), bad


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("golden", type=Path)
    ap.add_argument("config", type=Path, help="config.json of the runtime bundle")
    ap.add_argument("--cmd", required=True, help='engine command; "{config}" is replaced by the config path')
    ap.add_argument("--runs", help="comma-separated run names (default: all)")
    ap.add_argument("--show", type=int, default=20, help="differences to print per run")
    args = ap.parse_args()
    config = json.loads(args.config.read_text(encoding="utf-8"))
    base = args.config.resolve().parent
    runs = load_runs(args.golden, set(args.runs.split(",")) if args.runs else None)
    total_bad = 0
    for name, run in runs.items():
        n, bad = check_run(name, run, args.cmd, config, base, args.show)
        total_bad += bad
        print(f"{name}: {n} requests, {bad} differences", flush=True)
    print("PASS" if not total_bad else f"FAIL: {total_bad} differences")
    return 1 if total_bad else 0


if __name__ == "__main__":
    sys.exit(main())
