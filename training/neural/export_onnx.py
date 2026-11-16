"""Export a trained model for the engine: ONNX (float and 8-bit) plus its tables.

Usage:
    uv run python neural/export_onnx.py CHECKPOINT DATA_DIR OUT_DIR

Writes:
    OUT_DIR/neural.onnx        float32 model
    OUT_DIR/neural.int8.onnx   dynamically quantised (weights as 8-bit integers)
    OUT_DIR/neural.json        characters, syllable -> (base id, tone id), candidates

Model interface (one sequence, no padding):
    inputs   chars [N] int64   character id, or `mask_char` at positions to convert
             bases [N] int64   syllable base id (0 at context positions)
             tones [N] int64   tone id 0..4, or 5 for "unknown"
    output   logits [N, n_chars] float32   meaningful at the positions to convert
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import torch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from model import TONE_UNKNOWN, split_syllable  # noqa: E402
from score_testset import load_model  # noqa: E402


class Wrapper(torch.nn.Module):
    def __init__(self, model):
        super().__init__()
        self.model = model

    def forward(self, chars, bases, tones):
        lengths = torch.tensor([1], dtype=torch.long) * chars.shape[0]
        hidden = self.model(chars[None], bases[None], tones[None], lengths)
        return self.model.logits(hidden[0])


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("checkpoint", type=Path)
    ap.add_argument("data_dir", type=Path)
    ap.add_argument("out_dir", type=Path)
    args = ap.parse_args()

    model, tables, step = load_model(args.checkpoint, args.data_dir, torch.device("cpu"))
    args.out_dir.mkdir(parents=True, exist_ok=True)
    n = 24
    example = (torch.randint(0, 100, (n,)), torch.randint(0, 100, (n,)), torch.zeros(n, dtype=torch.long))
    fp32 = args.out_dir / "neural.onnx"
    torch.onnx.export(Wrapper(model), example, str(fp32), input_names=["chars", "bases", "tones"],
                      output_names=["logits"], opset_version=17,
                      dynamic_axes={"chars": {0: "n"}, "bases": {0: "n"}, "tones": {0: "n"}, "logits": {0: "n"}})

    from onnxruntime.quantization import QuantType, quantize_dynamic
    quantize_dynamic(str(fp32), str(args.out_dir / "neural.int8.onnx"), weight_type=QuantType.QInt8)

    vocab = json.loads((args.data_dir / "vocab.json").read_text(encoding="utf-8"))
    base_id = {b: i for i, b in enumerate(tables.bases)}
    meta = {
        "step": step, "max_len": model.cfg.max_len, "mask_char": tables.mask_char, "tone_unknown": TONE_UNKNOWN,
        "chars": tables.chars,
        "syllables": {s: [base_id[split_syllable(s)[0]], split_syllable(s)[1]] for s in tables.syllables[1:]},
        "candidates": vocab["candidates"],
    }
    (args.out_dir / "neural.json").write_text(json.dumps(meta, ensure_ascii=False), encoding="utf-8")
    for f in sorted(args.out_dir.iterdir()):
        print(f"{f.name}\t{f.stat().st_size / 1e6:.1f} MB")


if __name__ == "__main__":
    main()
