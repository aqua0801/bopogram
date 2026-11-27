"""Assemble a model pack (prototype/docs/release-spec.md). Plain Python 3, no packages.

Usage:
    python tools/make_model_pack.py --version 2026.12 --engine ">=0.2" \\
        --runtime RUNTIME_DIR --bglm BGLM_DIR --moe MOE_DIR --extra DICT_EXTRA_TSV OUT_DIR

RUNTIME_DIR  the runtime bundle: config.json, charlm/, reading_prior.tsv
BGLM_DIR     the n-gram models in the engine's format (bopogram build-lm), one per
             model in config.json ("models/ly.binary" -> ly.bglm)
MOE_DIR      the MOE archives exactly as downloaded (dict_*.zip) and their usage
             notes (*dict_*.pdf); copied byte for byte
DICT_EXTRA_TSV  build_dict.py extra's output

Writes OUT_DIR/bopogram-models-VERSION/ (the layout of the spec, with
manifest.json listing every file's size and SHA-256) and
OUT_DIR/bopogram-models-VERSION.zip. NOTICE.txt, LICENSE-models.txt and
selftest.json are taken from prototype/release/. The runtime's dict.tsv is
never copied: the dictionary is built on the user's machine.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import shutil
import sys
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parents[1] / "release"
MOE_ARCHIVES = ("dict_revised_*.zip", "dict_concised_*.zip", "dict_idioms_*.zip")
MOE_NOTES = ("reviseddict_*.pdf", "conciseddict_*.pdf", "idiomsdict_*.pdf")


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for block in iter(lambda: fh.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def one(folder: Path, pattern: str) -> Path:
    hits = sorted(folder.glob(pattern))
    if len(hits) != 1:
        sys.exit(f"expected exactly one {pattern} in {folder}, found {len(hits)}")
    return hits[0]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--version", required=True)
    ap.add_argument("--engine", required=True, help='lowest program version that reads this pack, e.g. ">=0.2"')
    ap.add_argument("--runtime", type=Path, required=True)
    ap.add_argument("--bglm", type=Path, required=True)
    ap.add_argument("--moe", type=Path, required=True)
    ap.add_argument("--extra", type=Path, required=True)
    args = ap.parse_args()

    name = f"bopogram-models-{args.version}"
    root = args.out_dir / name
    if root.exists():
        sys.exit(f"{root} exists")
    config = json.loads((args.runtime / "config.json").read_text(encoding="utf-8"))
    copies: list[tuple[Path, str]] = [
        (HERE / "NOTICE.txt", "NOTICE.txt"),
        (HERE / "LICENSE-models.txt", "LICENSE-models.txt"),
        (HERE / "selftest.json", "selftest.json"),
        (args.runtime / "config.json", "config.json"),
        (args.runtime / config["reading_prior"], config["reading_prior"]),
        (args.extra, "dict-source/dict_extra.tsv"),
    ]
    charlm = Path(config["charlm"]["model"])
    copies.append((args.runtime / charlm, str(charlm)))
    table = (args.runtime / charlm).with_suffix(".json")
    if not table.exists():
        table = (args.runtime / charlm).with_name("charlm.json")
    copies.append((table, str(charlm.with_name(table.name))))
    for m in config["models"]:
        stem = Path(m["path"]).stem
        copies.append((one(args.bglm, f"{stem}.bglm"), f"models/{stem}.bglm"))
    for pattern in MOE_ARCHIVES + MOE_NOTES:
        src = one(args.moe, pattern)
        copies.append((src, f"dict-source/{src.name}"))

    files = []
    for src, rel in copies:
        dest = root / rel
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(src, dest)
        files.append({"path": rel, "size": dest.stat().st_size, "sha256": sha256(dest)})
    manifest = {"format": 1, "name": "bopogram-models", "version": args.version, "engine": args.engine,
                "created": datetime.date.today().isoformat(), "files": files}
    (root / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    archive = args.out_dir / f"{name}.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as z:
        for path in sorted(root.rglob("*")):
            if path.is_file():
                z.write(path, f"{name}/{path.relative_to(root).as_posix()}")
    total = sum(f["size"] for f in files)
    print(f"{root}: {len(files)} files, {total / 1e6:.0f} MB; {archive.name} {archive.stat().st_size / 1e6:.0f} MB")


if __name__ == "__main__":
    main()
