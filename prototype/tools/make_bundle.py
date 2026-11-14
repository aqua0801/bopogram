"""Collect the files the engine needs at run time into one folder.

Usage:
    uv run python tools/make_bundle.py [--config CONFIG_JSON] OUT_DIR

Copies the dictionary and the language models named in the engine config and
writes a config.json with paths relative to the bundle, so the folder can be
copied to another machine (for example to Windows for the phase-4 trial).

The dictionary is built from Ministry of Education data (CC BY-ND): the
bundle is for your own machines only and must not be redistributed.
"""

from __future__ import annotations

import argparse
import json
import shutil
from pathlib import Path

from bopogram.engine import config_path


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--config")
    args = ap.parse_args()

    src = config_path(args.config)
    cfg = json.loads(src.read_text(encoding="utf-8"))
    out = args.out_dir
    (out / "models").mkdir(parents=True, exist_ok=True)
    shutil.copy2((src.parent / cfg["dict"]).resolve(), out / "dict.tsv")
    total = (out / "dict.tsv").stat().st_size
    for m in cfg["models"]:
        model = (src.parent / m["path"]).resolve()
        shutil.copy2(model, out / "models" / model.name)
        total += model.stat().st_size
        m["path"] = f"models/{model.name}"
    cfg["dict"] = "dict.tsv"
    cfg["user_model"] = "userdata/user.json"  # created on first use, stays on this machine
    (out / "config.json").write_text(json.dumps(cfg, ensure_ascii=False, indent=1), encoding="utf-8")
    (out / "DO_NOT_REDISTRIBUTE.txt").write_text(
        "dict.tsv is derived from Ministry of Education dictionaries (CC BY-ND 3.0 TW).\n"
        "This folder is for personal use on your own machines only.\n", encoding="utf-8")
    print(f"bundle written to {out} ({total / 1e6:.0f} MB)")


if __name__ == "__main__":
    main()
