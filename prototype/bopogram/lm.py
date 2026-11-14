"""Language model: a weighted mixture of KenLM n-gram models.

Scores are natural-log probabilities. The mixture is a linear interpolation,
P(w | h) = sum_i weight_i * P_i(w | h), so each hypothesis carries one KenLM
state per component model.

The token mapping below must stay identical to training/lm/prepare.py.
"""

from __future__ import annotations

import json
import math
import re
from pathlib import Path

import kenlm

NUM, LATIN, UNKNOWN_CHAR, PUNCT = "<n>", "<e>", "<x>", "<p>"
_RE_NUM = re.compile(r"^[0-9][0-9.%+\-]*$")
_RE_LATIN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9.%+\-]*$")
_RE_CJK = re.compile(r"^[一-鿿㐀-䶿]+$")
_LOG10 = math.log(10.0)
PREFERRED_FORMS = str.maketrans({
    "臺": "台", "裏": "裡", "爲": "為", "啓": "啟", "綫": "線", "衞": "衛", "峯": "峰",
    "羣": "群", "敍": "敘", "説": "說", "産": "產", "奬": "獎", "滙": "匯", "恒": "恆", "麽": "麼",
})

State = tuple  # tuple of kenlm.State, one per component


class MixtureLM:
    def __init__(self, model_paths: list[str | Path], weights: list[float] | None = None):
        if not model_paths:
            raise ValueError("at least one model is required")
        self.models = [kenlm.Model(str(p)) for p in model_paths]
        n = len(self.models)
        weights = weights or [1.0 / n] * n
        if len(weights) != n:
            raise ValueError("one weight per model is required")
        total = sum(weights)
        self.weights = [w / total for w in weights]
        self._single = n == 1
        self._cache: dict[tuple[State, str], tuple[float, State]] = {}

    @classmethod
    def from_config(cls, config_path: str | Path) -> "MixtureLM":
        """Config JSON: {"models": [{"path": "...", "weight": 0.5}, ...]}; paths relative to the file."""
        config_path = Path(config_path)
        cfg = json.loads(config_path.read_text(encoding="utf-8"))
        paths = [(config_path.parent / m["path"]).resolve() for m in cfg["models"]]
        return cls(paths, [m["weight"] for m in cfg["models"]])

    # -- states ---------------------------------------------------------
    def begin(self) -> State:
        states = []
        for m in self.models:
            s = kenlm.State()
            m.BeginSentenceWrite(s)
            states.append(s)
        return tuple(states)

    def score(self, state: State, token: str) -> tuple[float, State]:
        """ln P(token | state) and the following state."""
        key = (state, token)
        hit = self._cache.get(key)
        if hit is not None:
            return hit
        out = []
        if self._single:
            o = kenlm.State()
            logp = self.models[0].BaseScore(state[0], token, o) * _LOG10
            out.append(o)
        else:
            p = 0.0
            for m, w, s in zip(self.models, self.weights, state):
                o = kenlm.State()
                p += w * 10.0 ** m.BaseScore(s, token, o)
                out.append(o)
            logp = math.log(p) if p > 0.0 else -99.0
        res = (logp, tuple(out))
        if len(self._cache) > 2_000_000:
            self._cache.clear()
        self._cache[key] = res
        return res

    def unigram(self, token: str) -> float:
        """Context-free ln P(token); used to rank candidates and to segment context."""
        null = []
        for m in self.models:
            s = kenlm.State()
            m.NullContextWrite(s)
            null.append(s)
        return self.score(tuple(null), token)[0]

    # -- token mapping ----------------------------------------------------
    @staticmethod
    def map_token(tok: str, in_vocab) -> str:
        tok = tok.translate(PREFERRED_FORMS)
        if in_vocab(tok):
            return tok
        if _RE_NUM.match(tok):
            return NUM
        if _RE_LATIN.match(tok):
            return LATIN
        if _RE_CJK.match(tok):
            return UNKNOWN_CHAR
        return PUNCT
