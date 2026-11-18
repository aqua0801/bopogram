"""Export the character language model for the engine: ONNX (float and 8-bit) plus its table.

Usage:
    uv run python neural/export_charlm.py CHECKPOINT OUT_DIR

Writes:
    OUT_DIR/charlm.onnx        float32 model
    OUT_DIR/charlm.int8.onnx   dynamically quantised (weights as 8-bit integers)
    OUT_DIR/charlm.json        characters, special ids, whether lines end with <s>

Model interface:
    input    ids     [B, N] int64   <s> + context + candidate (+ <s>), right-padded with 0
    output   logp    [B, N-1] float32   ln P(ids[:, i+1] | ids[:, :i+1])
The output is already reduced to the probability of the next given character,
so the engine never receives the full [B, N, vocabulary] scores.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import torch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from charlm import BOS, load_lm  # noqa: E402


class Wrapper(torch.nn.Module):
    def __init__(self, model):
        super().__init__()
        self.model = model

    def forward(self, ids):
        logp = torch.log_softmax(self.model(ids[:, :-1]), dim=-1)
        return logp.gather(2, ids[:, 1:, None])[:, :, 0]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("checkpoint", type=Path)
    ap.add_argument("out_dir", type=Path)
    args = ap.parse_args()

    model, step = load_lm(args.checkpoint, torch.device("cpu"))
    ckpt = torch.load(args.checkpoint, map_location="cpu", weights_only=False)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    example = torch.randint(10, 100, (3, 20))
    with torch.no_grad():
        ref = Wrapper(model)(example).numpy()  # before export: tracing may change the module's state
    fp32 = args.out_dir / "charlm.onnx"
    torch.onnx.export(Wrapper(model), (example,), str(fp32), input_names=["ids"], output_names=["logp"],
                      opset_version=17, dynamo=False,
                      dynamic_axes={"ids": {0: "b", 1: "n"}, "logp": {0: "b", 1: "m"}})

    from onnxruntime.quantization import QuantType, quantize_dynamic
    quantize_dynamic(str(fp32), str(args.out_dir / "charlm.int8.onnx"), weight_type=QuantType.QInt8)

    meta = {"step": step, "max_len": model.cfg.max_len, "chars": ckpt["chars"],
            "bos": ckpt["chars"].index(BOS), "eos": bool(ckpt.get("eos"))}
    (args.out_dir / "charlm.json").write_text(json.dumps(meta, ensure_ascii=False), encoding="utf-8")

    # The exported model must agree with PyTorch.
    import numpy as np
    import onnxruntime as ort
    for name in ("charlm.onnx", "charlm.int8.onnx"):
        got = ort.InferenceSession(str(args.out_dir / name)).run(None, {"ids": example.numpy()})[0]
        print(f"{name}\t{(args.out_dir / name).stat().st_size / 1e6:.1f} MB\t"
              f"max |difference| per character {np.abs(got - ref).max():.3f}")


if __name__ == "__main__":
    main()
