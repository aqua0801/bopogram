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
from .user import UserModel

DEFAULT_CONFIG = Path.home() / "bopogram-data" / "models" / "config.json"


def config_path(explicit: str | None = None) -> Path:
    return Path(explicit or os.environ.get("BOPOGRAM_CONFIG") or DEFAULT_CONFIG)


class Engine:
    """Config JSON (paths relative to the file):

    {"dict": "../dict/dict.tsv",
     "models": [{"path": "ly.binary", "weight": 0.4}, ...],
     "beam": 16, "max_candidates": 40, "word_penalty": 0.0, "eos_weight": 0.0,
     "user_model": null}
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
        self.decoder = Decoder(self.lexicon, self.lm, beam=cfg.get("beam", 16),
                               max_candidates=cfg.get("max_candidates", 40),
                               extra_score=self.user.bonus if self.user else None,
                               word_penalty=cfg.get("word_penalty", 0.0),
                               eos_weight=cfg.get("eos_weight", 0.0))

    def start(self, context: str = "") -> None:
        """Begin a composition; `context` is the text before the cursor (never modified)."""
        self.decoder.reset(context_state(context, self.lexicon, self.lm))

    def convert(self, syllables: list[str], context: str = "", nbest: int = 1) -> list[tuple[str, float]]:
        self.start(context)
        for s in syllables:
            self.decoder.push(s)
        return [("".join(words), score) for words, score in self.decoder.nbest(nbest)]

    def convert_keys(self, keys: str, layout: str = "dachen", **kw) -> list[tuple[str, float]]:
        return self.convert(LAYOUTS[layout].to_syllables(keys), **kw)
