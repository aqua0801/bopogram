"""The character language model run without ONNX Runtime: the reference
implementation of the engine's own format (prototype/docs/charlm-native.md).

Plain numpy. Matrix products use 8-bit integers on both sides (weights stored
that way, activations quantized per position just before each product); they
are done in float64, where every integer sum is exact, so the Rust engine's
integer arithmetic gives the same numbers. Tensors are float32 between steps;
inside each step (layer norm, attention, softmax, GELU, dequantization) the
arithmetic is float64 and the result is rounded to float32 once. Rounding to
8 bits magnifies the last bit of a float32 now and then, so the order of
float32 additions would make two implementations drift apart; float64 inside a
step hides the order almost always.

Prefix cache: a causal model's state at a position depends only on the text up
to it, so the keys and values computed for one sequence serve every later
sequence that starts the same way. The candidates scored together share the
context and most of the composition, and the next keystroke shares almost all
of it again, so only the characters that differ are computed.
"""

from __future__ import annotations

import json
import struct
from pathlib import Path

import numpy as np

MAGIC = b"BPCLM\x00\x00\x01"
ALIGN = 64
PREFIX_CACHE = 32  # sequences whose state is kept

F32 = np.float32
F64 = np.float64


def round_half_away(x: np.ndarray) -> np.ndarray:
    return np.sign(x) * np.floor(np.abs(x) + 0.5)


def erf(x: np.ndarray) -> np.ndarray:
    """Abramowitz & Stegun 7.1.26 (|error| < 1.5e-7), in float64; part of the format."""
    s = np.sign(x)
    a = np.abs(x)
    t = 1.0 / (1.0 + 0.3275911 * a)
    y = ((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t
    return s * (1.0 - y * np.exp(-a * a))


def gelu(x: np.ndarray) -> np.ndarray:
    x = x.astype(F64)
    return (0.5 * x * (1.0 + erf(x * 0.7071067811865476))).astype(F32)


def layer_norm(x: np.ndarray, w: np.ndarray, b: np.ndarray, eps: float) -> np.ndarray:
    x = x.astype(F64)
    mean = x.mean(axis=-1, keepdims=True)
    var = ((x - mean) ** 2).mean(axis=-1, keepdims=True)
    return ((x - mean) / np.sqrt(var + eps) * w + b).astype(F32)


def quantize(x: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """Per row of float32: (q as float64 integers in [-127, 127], scale as float32)."""
    scale = (np.abs(x).max(axis=-1).astype(F64) / 127.0).astype(F32)
    safe = np.where(scale == 0, F32(1), scale).astype(F64)
    q = np.clip(round_half_away(x.astype(F64) / safe[:, None]), -127, 127)
    return q, scale


def attention(q: np.ndarray, k: np.ndarray, v: np.ndarray, visible: np.ndarray, dh: int) -> np.ndarray:
    """One head. Rounded to float32 after: the scaled scores, the exponentials,
    the probabilities, and the output."""
    s = ((q.astype(F64) @ k.T.astype(F64)).astype(F32).astype(F64) / np.sqrt(dh)).astype(F32).astype(F64)
    s = np.where(visible, s, -np.inf)
    e = np.exp(s - s.max(axis=1, keepdims=True)).astype(F32).astype(F64)
    p = (e / e.sum(axis=1, keepdims=True)).astype(F32).astype(F64)
    return (p @ v.astype(F64)).astype(F32)


class QLinear:
    def __init__(self, q: np.ndarray, scale: np.ndarray, bias: np.ndarray | None):
        # Integer sums below 2**24 are exact in float32 whatever the order, which
        # holds for up to 1040 inputs (127 * 127 * 1040 < 2**24); float32 is faster.
        exact32 = q.shape[1] * 127 * 127 < 2 ** 24
        self.qt = q.astype(F32 if exact32 else F64).T.copy()  # [in, out]
        self.scale = scale.astype(F64)
        self.bias = bias.astype(F64) if bias is not None else None

    def __call__(self, x: np.ndarray) -> np.ndarray:
        qx, sx = quantize(x)
        acc = (qx.astype(self.qt.dtype) @ self.qt).astype(F64)  # exact integers
        y = acc * (sx.astype(F64)[:, None] * self.scale[None, :])
        if self.bias is not None:
            y = y + self.bias
        return y.astype(F32)


class NativeCharLM:
    """Same interface as charlm.CharLMScorer._logp: log-probabilities of sequences."""

    def __init__(self, path: str | Path):
        raw = Path(path).read_bytes()
        if raw[:8] != MAGIC:
            raise ValueError(f"{path}: not a character model in the engine's format")
        (n,) = struct.unpack("<I", raw[8:12])
        head = json.loads(raw[12:12 + n].decode("utf-8"))
        start = (12 + n + ALIGN - 1) // ALIGN * ALIGN
        t = {}
        for e in head["tensors"]:
            dt = np.int8 if e["dtype"] == "i8" else F32
            count = int(np.prod(e["shape"]))
            t[e["name"]] = np.frombuffer(raw, dt, count, start + e["offset"]).reshape(e["shape"])
        self.d, self.h = head["d_model"], head["n_heads"]
        self.eps, self.max_len = head["ln_eps"], head["max_len"]
        self.emb = t["emb.q"].astype(F32) * t["emb.scale"][:, None]  # input lookup
        self.out = QLinear(t["emb.q"], t["emb.scale"], t["out_bias"])
        self.pos = t["pos"]
        self.blocks = []
        for i in range(head["n_layers"]):
            g = lambda k: t[f"{i}.{k}"]
            self.blocks.append({
                "n1": (g("norm1.w"), g("norm1.b")), "n2": (g("norm2.w"), g("norm2.b")),
                **{k: QLinear(g(k + ".q"), g(k + ".scale"), g(k + ".b")) for k in ("qkv", "proj", "ff1", "ff2")},
            })
        self.norm = (t["norm.w"], t["norm.b"])
        self._cache: list[tuple[tuple[int, ...], dict]] = []

    # -- prefix cache --------------------------------------------------------
    def _state(self, seq: tuple[int, ...]) -> tuple[dict, int]:
        """The cached state sharing the longest prefix with `seq`, and that length."""
        best, n = None, 0
        for key, st in self._cache:
            k = 0
            for a, b in zip(key, seq):
                if a != b:
                    break
                k += 1
            if k > n:
                best, n = st, k
        return best, n

    def _extend(self, seq: tuple[int, ...]) -> dict:
        """State for every position of `seq`: per layer K and V, and the final
        normalized hidden vector."""
        old, n = self._state(seq)
        n = min(n, len(seq))
        if old is not None and n == len(seq) and len(old["x"]) == n:
            return old
        d, h = self.d, self.h
        dh = d // h
        ids = np.array(seq[n:])
        x = self.emb[ids] + self.pos[n:len(seq)]
        ks, vs = [], []
        for li, blk in enumerate(self.blocks):
            qkv = blk["qkv"](layer_norm(x, *blk["n1"], self.eps))
            q, k, v = qkv[:, :d], qkv[:, d:2 * d], qkv[:, 2 * d:]
            if old is not None and n:
                k = np.concatenate([old["k"][li][:n], k])
                v = np.concatenate([old["v"][li][:n], v])
            ks.append(k)
            vs.append(v)
            m, total = len(ids), len(k)
            a = np.empty((m, d), dtype=F32)
            # Causal: new position j (absolute n + j) sees keys 0 .. n + j.
            visible = np.arange(total)[None, :] <= (n + np.arange(m))[:, None]
            for hd in range(h):
                sl = slice(hd * dh, (hd + 1) * dh)
                a[:, sl] = attention(q[:, sl], k[:, sl], v[:, sl], visible, dh)
            x = x + blk["proj"](a)
            x = x + blk["ff2"](gelu(blk["ff1"](layer_norm(x, *blk["n2"], self.eps))))
        xn = layer_norm(x, *self.norm, self.eps)
        if old is not None and n:
            xn = np.concatenate([old["x"][:n], xn])
        st = {"k": ks, "v": vs, "x": xn}
        self._cache.append((seq, st))
        if len(self._cache) > PREFIX_CACHE:
            self._cache.pop(0)
        return st

    def logp(self, seq: list[int], first: int) -> float:
        """Sum of ln P(seq[i + 1] | seq[: i + 1]) for i = first .. len(seq) - 2."""
        st = self._extend(tuple(seq))
        logits = self.out(st["x"][first:len(seq) - 1]).astype(F64)
        top = logits.max(axis=1, keepdims=True)
        lse = np.log(np.exp(logits - top).sum(axis=1)) + top[:, 0]
        target = logits[np.arange(len(lse)), np.array(seq[first + 1:])]
        return float((target - lse).sum())
