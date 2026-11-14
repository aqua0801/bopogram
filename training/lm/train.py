"""Train one KenLM model per corpus group.

Usage:
    uv run python lm/train.py SEGMENTED_DIR DICT_TSV MODEL_DIR --kenlm KENLM_BIN_DIR

Groups (each becomes MODEL_DIR/<group>.binary):
    ly       Legislative Yuan transcripts (spoken)
    wiki     Wikipedia
    judicial court judgments
    general  press releases, Wikinews, user prompts

Order 3, modified Kneser-Ney, singleton trigrams pruned, quantised trie.
lmplz memory is capped (-S) and its temporary files go to MODEL_DIR/tmp.
Resume: a group whose .binary exists is skipped; the ARPA file is kept so
later tools (interpolation, size experiments) do not need to retrain.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

GROUPS = {
    "ly": ["ly_gazette"],
    "wiki": ["zhwiki"],
    "judicial": ["judicial"],
    "general": ["ey", "ey_news", "zhwikinews", "twllm"],
}
HERE = Path(__file__).resolve().parent


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("segmented_dir", type=Path)
    ap.add_argument("dict_tsv", type=Path)
    ap.add_argument("model_dir", type=Path)
    ap.add_argument("--kenlm", type=Path, required=True, help="directory with lmplz and build_binary")
    ap.add_argument("--memory", default="10G")
    ap.add_argument("--only", nargs="*", help="train only these groups")
    args = ap.parse_args()
    args.model_dir.mkdir(parents=True, exist_ok=True)
    tmp = args.model_dir / "tmp"
    tmp.mkdir(exist_ok=True)

    for group, sources in GROUPS.items():
        if args.only and group not in args.only:
            continue
        binary = args.model_dir / f"{group}.binary"
        if binary.exists():
            print(f"{group}: already trained", flush=True)
            continue
        arpa = args.model_dir / f"{group}.arpa"
        dirs = [str(args.segmented_dir / s) for s in sources]
        prune = ["--prune", "0", "0", "1"] if group != "general" else []
        prep = subprocess.Popen([sys.executable, str(HERE / "prepare.py"), str(args.dict_tsv), *dirs],
                                stdout=subprocess.PIPE)
        with open(arpa.with_suffix(".tmp"), "wb") as out, open(args.model_dir / f"{group}.lmplz.log", "wb") as log:
            lm = subprocess.run([str(args.kenlm / "lmplz"), "-o", "3", "-S", args.memory, "-T", str(tmp),
                                 "--discount_fallback", *prune], stdin=prep.stdout, stdout=out, stderr=log)
        prep.stdout.close()
        if prep.wait() != 0 or lm.returncode != 0:
            sys.exit(f"{group}: training failed, see {group}.lmplz.log")
        arpa.with_suffix(".tmp").rename(arpa)
        with open(args.model_dir / f"{group}.build.log", "wb") as log:
            subprocess.run([str(args.kenlm / "build_binary"), "-q", "8", "-b", "8", "-a", "22", "trie",
                            str(arpa), str(binary.with_suffix(".tmp"))], check=True, stdout=log, stderr=log)
        binary.with_suffix(".tmp").rename(binary)
        print(f"{group}: {binary.stat().st_size / 1e6:.0f} MB binary, "
              f"{arpa.stat().st_size / 1e6:.0f} MB arpa", flush=True)


if __name__ == "__main__":
    main()
