"""Neural position-wise scores (optional): an ONNX model run on the CPU.

The model reads the left context (characters) and the composition buffer
(syllables) and returns, for every syllable, a log-probability for each
character that syllable can stand for. The decoder adds these to its lattice
(Decoder.set_position_scores); the n-gram engine works without them.

Nothing here uses the network: the model is a local file run by ONNX Runtime.
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np

from .decoder import is_literal

MAX_CONTEXT = 40


class NeuralScorer:
    def __init__(self, model_path: str | Path, threads: int = 2):
        import onnxruntime as ort

        model_path = Path(model_path)
        meta = json.loads(model_path.with_name("neural.json").read_text(encoding="utf-8"))
        opts = ort.SessionOptions()
        opts.intra_op_num_threads = threads
        opts.inter_op_num_threads = 1
        self._session = ort.InferenceSession(str(model_path), opts, providers=["CPUExecutionProvider"])
        self._char_id = {c: i for i, c in enumerate(meta["chars"])}
        self._chars = meta["chars"]
        self._mask = meta["mask_char"]
        self._syllables: dict[str, list[int]] = meta["syllables"]
        self._max_len = meta["max_len"]
        self._cands = {s: (chars, np.array([self._char_id[c] for c in chars], dtype=np.int64))
                       for s, chars in meta["candidates"].items() if chars}
        # Spellings the dictionary accepts but the model was not trained with
        # (see training/dict/typing_variants.tsv) are scored as the standard reading.
        self.aliases: dict[str, str] = {"˙ㄇㄛ": "˙ㄇㄜ"}
        # Tone-sandhi spellings of 一 / 不 were not in the training data either; the
        # model has no opinion on them (they get the score of its best candidate).
        self.sandhi = {"ㄧˊ": "一", "ㄧˋ": "一", "ㄅㄨˊ": "不"}

    def _context_ids(self, text: str) -> list[int]:
        """Same encoding as training/neural/make_data.py."""
        cid, out, run = self._char_id, [], None
        for ch in text[-MAX_CONTEXT:]:
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

    def score(self, context: str, items: list[str]) -> list[dict[str, float]]:
        """One dict per buffer item: character -> log-probability.
        Literals and syllables the model does not know get an empty dict (no opinion)."""
        chars = self._context_ids(context)
        bases, tones = [0] * len(chars), [0] * len(chars)
        known: list[str | None] = []
        for item in items:
            if is_literal(item):
                ch = item[1:]
                chars.extend(self._context_ids(ch) or [self._char_id["<p>"]])
                bases.append(0)
                tones.append(0)
                known.append(None)
                continue
            syl = self.aliases.get(item, item)
            entry = self._syllables.get(syl)
            if entry is None or syl not in self._cands:
                chars.append(self._char_id["<x>"])
                bases.append(0)
                tones.append(0)
                known.append(None)
            else:
                chars.append(self._mask)
                bases.append(entry[0])
                tones.append(entry[1])
                known.append(syl)
        if not any(known):
            return [{} for _ in items]
        cut = max(0, len(chars) - self._max_len)  # keep the end: the buffer matters most
        logits = self._session.run(None, {"chars": np.array(chars[cut:], dtype=np.int64),
                                          "bases": np.array(bases[cut:], dtype=np.int64),
                                          "tones": np.array(tones[cut:], dtype=np.int64)})[0]
        first = len(chars) - len(items) - cut
        out: list[dict[str, float]] = []
        for k, syl in enumerate(known):
            row = first + k
            if syl is None or row < 0:
                out.append({})
                continue
            cand_chars, ids = self._cands[syl]
            x = logits[row, ids].astype(np.float64)
            x -= x.max()
            logp = x - np.log(np.exp(x).sum())
            scores = dict(zip(cand_chars, logp.tolist()))
            if syl in self.sandhi:
                scores[self.sandhi[syl]] = 0.0
            out.append(scores)
        return out
