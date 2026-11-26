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


def encode(text: str, cid: dict[str, int]) -> list[int]:
    """Character ids, as training/neural/make_data.py writes them: a run of digits is
    one <n>, a run of Latin letters one <e>, spaces are dropped, an unknown Chinese
    character is <x> and anything else unknown <p>."""
    out, run = [], None
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


def sequence(context: str, text: str, cid: dict[str, int], bos: int, eos: bool, max_len: int) -> tuple[list[int], int]:
    """The ids scored for `text` after `context`, and the first scored output position:
    the sum of ln P(seq[i + 1] | seq[..i]) from that position on is the text's score."""
    prefix = [bos] + encode(context[-MAX_CONTEXT:], cid)
    body = encode(text, cid) + ([bos] if eos else [])
    seq = (prefix + body)[-max_len:]
    return seq, max(0, len(seq) - len(body) - 1)


def table_path(model_path: Path) -> Path:
    """The model's character table: <model name>.json beside it (charlm-lite.bin ->
    charlm-lite.json), else charlm.json in the same folder."""
    own = model_path.with_suffix(".json")
    return own if own.exists() else model_path.with_name("charlm.json")


class CharLMScorer:
    def __init__(self, model_path: str | Path, threads: int = 2):
        model_path = Path(model_path)
        meta = json.loads(table_path(model_path).read_text(encoding="utf-8"))
        self.rerank: dict = meta.get("rerank", {})  # this model's tuned weight and top
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
        return encode(text, self._char_id)

    def score(self, context: str, texts: list[str]) -> list[float]:
        """ln P(text, and the end of the line after it | context) for each text."""
        context = context[-MAX_CONTEXT:]
        missing = [t for t in dict.fromkeys(texts) if (context, t) not in self._cache]
        if missing:
            seqs, firsts = [], []
            for t in missing:
                seq, first = sequence(context, t, self._char_id, self._bos, self._eos, self._max_len)
                seqs.append(seq)
                firsts.append(first)
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
