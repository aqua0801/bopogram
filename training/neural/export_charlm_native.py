"""Export the character language model in the engine's own format, for
inference without ONNX Runtime (prototype/bopogram/charlm_native.py, and the
Rust engine). The format is described in prototype/docs/charlm-native.md.

Usage:
    uv run python neural/export_charlm_native.py CHECKPOINT OUT_DIR

Writes OUT_DIR/charlm.bin (weights) and OUT_DIR/charlm.json (characters and
special ids; the same table export_charlm.py writes).

Every matrix that multiplies activations (the four linear layers of each block,
and the character embedding, which is also the output layer) is stored as 8-bit
integers per row with one float32 scale per row:
    scale = max |row| / 127  (1 for a row of zeros),  q = round(w / scale)
rounding halves away from zero. Everything else (position embedding, layer
norms, biases) stays float32.
"""

from __future__ import annotations

import argparse
import json
import struct
import sys
from pathlib import Path

import numpy as np
import torch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from charlm import BOS  # noqa: E402

MAGIC = b"BPCLM\x00\x00\x01"
ALIGN = 64


def quantize_rows(w: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    w = w.astype(np.float32)
    scale = (np.abs(w).max(axis=1) / np.float32(127)).astype(np.float32)
    scale[scale == 0] = 1
    x = w / scale[:, None]
    q = np.sign(x) * np.floor(np.abs(x) + np.float32(0.5))
    return np.clip(q, -127, 127).astype(np.int8), scale


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("checkpoint", type=Path)
    ap.add_argument("out_dir", type=Path)
    args = ap.parse_args()
    ckpt = torch.load(args.checkpoint, map_location="cpu", weights_only=False)
    cfg, sd = ckpt["config"], {k: v.float().numpy() for k, v in ckpt["model"].items()}

    tensors: list[tuple[str, np.ndarray]] = []

    def add_q(name: str, w: np.ndarray) -> None:
        q, s = quantize_rows(w)
        tensors.extend([(name + ".q", q), (name + ".scale", s)])

    add_q("emb", sd["char_emb.weight"])
    tensors.append(("pos", sd["pos_emb.weight"]))
    for i in range(cfg["n_layers"]):
        p = f"blocks.{i}."
        for norm in ("norm1", "norm2"):
            tensors += [(f"{i}.{norm}.w", sd[p + norm + ".weight"]), (f"{i}.{norm}.b", sd[p + norm + ".bias"])]
        for lin in ("qkv", "proj", "ff1", "ff2"):
            add_q(f"{i}.{lin}", sd[p + lin + ".weight"])
            tensors.append((f"{i}.{lin}.b", sd[p + lin + ".bias"]))
    tensors += [("norm.w", sd["norm.weight"]), ("norm.b", sd["norm.bias"]), ("out_bias", sd["out_bias"])]

    index, blobs, offset = [], [], 0
    for name, arr in tensors:
        arr = np.ascontiguousarray(arr.astype(np.int8 if arr.dtype == np.int8 else np.float32))
        offset = (offset + ALIGN - 1) // ALIGN * ALIGN
        index.append({"name": name, "dtype": "i8" if arr.dtype == np.int8 else "f32",
                      "shape": list(arr.shape), "offset": offset})
        blobs.append((offset, arr.tobytes()))
        offset += arr.nbytes
    header = json.dumps({"format": 1, "d_model": cfg["d_model"], "n_layers": cfg["n_layers"],
                         "n_heads": cfg["n_heads"], "d_ff": cfg["d_ff"], "n_chars": cfg["n_chars"],
                         "max_len": cfg["max_len"], "ln_eps": 1e-5, "step": ckpt["step"],
                         "tensors": index}).encode("utf-8")
    start = (len(MAGIC) + 4 + len(header) + ALIGN - 1) // ALIGN * ALIGN
    args.out_dir.mkdir(parents=True, exist_ok=True)
    out = args.out_dir / "charlm.bin"
    with open(out, "wb") as fh:
        fh.write(MAGIC + struct.pack("<I", len(header)) + header)
        for off, data in blobs:
            fh.seek(start + off)
            fh.write(data)
    meta = {"step": ckpt["step"], "max_len": cfg["max_len"], "chars": ckpt["chars"],
            "bos": ckpt["chars"].index(BOS), "eos": bool(ckpt.get("eos"))}
    (args.out_dir / "charlm.json").write_text(json.dumps(meta, ensure_ascii=False), encoding="utf-8")
    print(f"{out}\t{out.stat().st_size / 1e6:.1f} MB\t{len(tensors)} tensors")


if __name__ == "__main__":
    main()
