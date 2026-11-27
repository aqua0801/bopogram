"""Assemble a charlm tier pack (prototype/docs/release-spec.md): the extra
lite or advanced model for `bopogram pack add-tier`. Plain Python 3, no packages.

Usage:
    python make_tier_pack.py --tier lite --version 2026.12 --engine ">=0.2" \\
        --charlm RUNTIME_CHARLM_DIR --release PROTOTYPE_RELEASE_DIR OUT_DIR

Writes OUT_DIR/bopogram-charlm-TIER-VERSION.zip: charlm/charlm-TIER.bin and
.json, NOTICE.txt, LICENSE-models.txt (from prototype/release/) and
manifest.json (every file's size and SHA-256, paths with forward slashes).
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import sys
import zipfile
from pathlib import Path


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--tier", required=True, choices=["lite", "advanced"])
    ap.add_argument("--version", required=True)
    ap.add_argument("--engine", required=True)
    ap.add_argument("--charlm", type=Path, required=True)
    ap.add_argument("--release", type=Path, required=True)
    args = ap.parse_args()

    name = f"bopogram-charlm-{args.tier}-{args.version}"
    sources = [
        (args.charlm / f"charlm-{args.tier}.bin", f"charlm/charlm-{args.tier}.bin"),
        (args.charlm / f"charlm-{args.tier}.json", f"charlm/charlm-{args.tier}.json"),
        (args.release / "NOTICE.txt", "NOTICE.txt"),
        (args.release / "LICENSE-models.txt", "LICENSE-models.txt"),
    ]
    files = []
    for src, rel in sources:
        if not src.is_file():
            sys.exit(f"missing: {src}")
        data = src.read_bytes()
        files.append((rel, data, {"path": rel, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}))
    manifest = {"format": 1, "name": "bopogram-charlm", "version": args.version, "engine": args.engine,
                "tier": args.tier, "created": datetime.date.today().isoformat(), "files": [f[2] for f in files]}
    args.out_dir.mkdir(parents=True, exist_ok=True)
    archive = args.out_dir / f"{name}.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as z:
        z.writestr(f"{name}/manifest.json", json.dumps(manifest, ensure_ascii=False, indent=1) + "\n")
        for rel, data, _ in files:
            z.writestr(f"{name}/{rel}", data)
    print(f"{archive}: {sum(len(f[1]) for f in files) / 1e6:.0f} MB")


if __name__ == "__main__":
    main()
