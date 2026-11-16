"""Engine facade: configuration, loading, and one-call conversion."""

from __future__ import annotations

import json
import os
from pathlib import Path

from .context import context_state
from .decoder import Decoder
from .layouts import LAYOUTS
from .lexicon import Lexicon
from .lm import MixtureLM
from .typing_log import TypingLog
from .user import UserModel

DEFAULT_CONFIG = Path.home() / "bopogram-data" / "models" / "config.json"


def config_path(explicit: str | None = None) -> Path:
    return Path(explicit or os.environ.get("BOPOGRAM_CONFIG") or DEFAULT_CONFIG)


class Engine:
    """Config JSON (paths relative to the file):

    {"dict": "../dict/dict.tsv",
     "models": [{"path": "ly.binary", "weight": 0.4}, ...],
     "beam": 16, "max_candidates": 40, "word_penalty": 0.0, "eos_weight": 0.0,
     "style": "casual", "user_model": null, "typing_log": null,
     "neural": {"model": "neural/neural.int8.onnx", "weight": 0.3, "floor": -12.0, "threads": 2}}

    "style" is "casual" (a lone neutral-tone ㄉㄜ is always 的) or "formal"
    (的 / 地 / 得 are chosen by the model); the default is "casual".
    "typing_log" names a local file that records committed compositions for
    later review (see typing_log.py); it is off when absent or null.
    "neural" adds the scores of the position-wise model (see neural.py) to the
    lattice; absent or null means the n-gram engine alone.
    """

    def __init__(self, config: str | Path | None = None, use_user_model: bool = True):
        path = config_path(str(config) if config else None)
        cfg = json.loads(path.read_text(encoding="utf-8"))
        base = path.parent
        self.lexicon = Lexicon((base / cfg["dict"]).resolve())
        self.lm = MixtureLM([(base / m["path"]).resolve() for m in cfg["models"]],
                            [m["weight"] for m in cfg["models"]])
        self.user = None
        if use_user_model and cfg.get("user_model"):
            user_path = Path(cfg["user_model"]).expanduser()
            self.user = UserModel(user_path if user_path.is_absolute() else base / user_path)
        self.typing_log = None
        if use_user_model and cfg.get("typing_log"):
            log_path = Path(cfg["typing_log"]).expanduser()
            self.typing_log = TypingLog(log_path if log_path.is_absolute() else base / log_path)
        self.neural = None
        self.neural_weight, self.neural_floor = 0.0, -12.0
        if cfg.get("neural"):
            self.set_neural(**{**cfg["neural"], "model": (base / cfg["neural"]["model"]).resolve()})
        self._decoder_args = dict(beam=cfg.get("beam", 16), max_candidates=cfg.get("max_candidates", 40),
                                  word_penalty=cfg.get("word_penalty", 0.0),
                                  eos_weight=cfg.get("eos_weight", 0.0),
                                  style=cfg.get("style", "casual"))
        self.decoder = self.new_decoder()

    def set_neural(self, model, weight: float = 0.3, floor: float = -12.0, threads: int = 2) -> None:
        from .neural import NeuralScorer  # imported only when used: ONNX Runtime is optional

        self.neural = NeuralScorer(model, threads)
        self.neural_weight, self.neural_floor = weight, floor

    def rescore(self, decoder: Decoder, context: str) -> None:
        """Refresh a decoder's neural scores after its buffer changed (no-op without a model)."""
        if self.neural is not None:
            scores = self.neural.score(context, decoder.syllables) if decoder.syllables else None
            decoder.rescore(scores, self.neural_weight, self.neural_floor)

    def set_style(self, style: str) -> None:
        """Style for this engine's own decoder and for decoders created afterwards."""
        self.decoder.set_style(style)
        self._decoder_args["style"] = style

    def new_decoder(self) -> Decoder:
        """A decoder with its own buffer, sharing this engine's dictionary and models."""
        return Decoder(self.lexicon, self.lm, extra_score=self.user.bonus if self.user else None,
                       **self._decoder_args)

    def start(self, context: str = "") -> None:
        """Begin a composition; `context` is the text before the cursor (never modified)."""
        self.decoder.reset(context_state(context, self.lexicon, self.lm))

    def convert(self, syllables: list[str], context: str = "", nbest: int = 1) -> list[tuple[str, float]]:
        self.start(context)
        for s in syllables:
            self.decoder.push(s)
        self.rescore(self.decoder, context)
        return [("".join(words), score) for words, score in self.decoder.nbest(nbest)]

    def convert_keys(self, keys: str, layout: str = "dachen", **kw) -> list[tuple[str, float]]:
        return self.convert(LAYOUTS[layout].to_syllables(keys), **kw)
