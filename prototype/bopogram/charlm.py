"""Character language model (optional): re-ranks the decoder's best few results.

The model was trained on plain text only (training/neural/charlm.py). It reads
the text before the cursor and each candidate result, and says how natural the
candidate reads, including how it ends. The engine adds `weight` times that
log-probability to the n-gram score of each of the decoder's `top` best results
and shows the highest. It never adds a result the decoder did not produce, so
dictionary words and the user's picks stay in control.

Nothing here uses the network: the model is a local file, run either in the
engine's own format (charlm.bin, charlm_native.py) or by ONNX Runtime (.onnx).
"""

from __future__ import annotations

import json
from collections import OrderedDict
from pathlib import Path

import numpy as np

MAX_CONTEXT = 40
CACHE_SIZE = 2000


class CharLMScorer:
    def __init__(self, model_path: str | Path, threads: int = 2):
        model_path = Path(model_path)
        meta = json.loads(model_path.with_name("charlm.json").read_text(encoding="utf-8"))
        self._session = self._native = None
        if model_path.suffix == ".bin":  # the engine's own format: no ONNX Runtime
            from .charlm_native import NativeCharLM
            self._native = NativeCharLM(model_path)
        else:
            import onnxruntime as ort

            # Official builds send usage events through Windows' event tracing; turn
            # them off before any session exists (no network functions, CLAUDE.md).
            ort.disable_telemetry_events()
            opts = ort.SessionOptions()
            opts.intra_op_num_threads = threads
            opts.inter_op_num_threads = 1
            self._session = ort.InferenceSession(str(model_path), opts, providers=["CPUExecutionProvider"])
        self._char_id = {c: i for i, c in enumerate(meta["chars"])}
        self._bos = meta["bos"]
        self._eos = meta["eos"]
        self._max_len = meta["max_len"]
        # (context, text) -> score. Typing changes the buffer one item at a time, so
        # after Backspace, cursor moves or candidate browsing most results recur.
        self._cache: OrderedDict[tuple[str, str], float] = OrderedDict()

    def encode(self, text: str) -> list[int]:
        """Same encoding as training/neural/make_data.py."""
        cid, out, run = self._char_id, [], None
        for ch in text:
            kind = "<n>" if ch.isdigit() else "<e>" if ch.isascii() and ch.isalpha() else None
            if kind:
                if run != kind:
                    out.append(cid[kind])
                run = kind
                continue
            run = None
            if not ch.isspace():
                out.append(cid.get(ch, cid["<x>"] if "一" <= ch <= "鿿" else cid["<p>"]))
        return out

    def score(self, context: str, texts: list[str]) -> list[float]:
        """ln P(text, and the end of the line after it | context) for each text."""
        context = context[-MAX_CONTEXT:]
        missing = [t for t in dict.fromkeys(texts) if (context, t) not in self._cache]
        if missing:
            prefix = [self._bos] + self.encode(context)
            tail = [self._bos] if self._eos else []
            seqs, firsts = [], []
            for t in missing:
                body = self.encode(t) + tail
                seq = (prefix + body)[-self._max_len:]
                seqs.append(seq)
                firsts.append(max(0, len(seq) - len(body) - 1))  # first scored output position
            if self._native is not None:
                for t, s, a in zip(missing, seqs, firsts):
                    self._cache[(context, t)] = self._native.logp(s, a)
            else:
                n = max(len(s) for s in seqs)
                ids = np.zeros((len(seqs), n), dtype=np.int64)
                for i, s in enumerate(seqs):
                    ids[i, : len(s)] = s
                logp = self._session.run(None, {"ids": ids})[0]
                for i, (t, s, a) in enumerate(zip(missing, seqs, firsts)):
                    self._cache[(context, t)] = float(logp[i, a: len(s) - 1].sum())
        out = []
        for t in texts:
            key = (context, t)
            self._cache.move_to_end(key)
            out.append(self._cache[key])
        while len(self._cache) > CACHE_SIZE:  # only after reading: this call's results stay
            self._cache.popitem(last=False)
        return out
