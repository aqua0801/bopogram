"""Engine facade: configuration, loading, and one-call conversion."""

from __future__ import annotations

import gc
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
     "neural": {"model": "neural/neural.int8.onnx", "weight": 0.3, "floor": -12.0, "threads": 2},
     "charlm": {"model": "charlm/charlm.int8.onnx", "weight": 0.8, "top": 5, "threads": 2}}

    "style" is "casual" (a lone neutral-tone ㄉㄜ is always 的) or "formal"
    (的 / 地 / 得 are chosen by the model); the default is "casual".
    "typing_log" names a local file that records committed compositions for
    later review (see typing_log.py); it is off when absent or null.
    "reading_prior" names the (character, syllable, count) table that makes a
    character less likely under a reading it seldom has; optional.
    "neural" adds the scores of the position-wise model (see neural.py) to the
    lattice; absent or null means the n-gram engine alone.
    "charlm" re-ranks the decoder's `top` best results with the character language
    model (see charlm.py); absent or null means no re-ranking.
    """

    def __init__(self, config: str | Path | None = None, use_user_model: bool = True):
        path = config_path(str(config) if config else None)
        cfg = json.loads(path.read_text(encoding="utf-8"))
        base = path.parent
        self.lexicon = Lexicon((base / cfg["dict"]).resolve())
        if cfg.get("reading_prior"):
            self.lexicon.load_reading_prior((base / cfg["reading_prior"]).resolve(),
                                            cfg.get("reading_prior_weight", 1.0))
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
        self.charlm = None
        self.charlm_weight, self.charlm_top = 0.0, 5
        if cfg.get("charlm"):
            self.set_charlm(**{**cfg["charlm"], "model": (base / cfg["charlm"]["model"]).resolve()})
        self._decoder_args = dict(beam=cfg.get("beam", 16), max_candidates=cfg.get("max_candidates", 40),
                                  word_penalty=cfg.get("word_penalty", 0.0),
                                  eos_weight=cfg.get("eos_weight", 0.0),
                                  style=cfg.get("style", "casual"))
        self.decoder = self.new_decoder()
        # The dictionary is millions of long-lived Python objects. Without this, every
        # full garbage collection walks all of them and a keystroke now and then takes
        # over 100 ms. Freezing moves what exists now out of the collector's sight.
        gc.collect()
        gc.freeze()

    def set_neural(self, model, weight: float = 0.3, floor: float = -12.0, threads: int = 2) -> None:
        from .neural import NeuralScorer  # imported only when used: ONNX Runtime is optional

        self.neural = NeuralScorer(model, threads)
        self.neural_weight, self.neural_floor = weight, floor

    def set_charlm(self, model, weight: float = 0.8, top: int = 5, threads: int = 2) -> None:
        from .charlm import CharLMScorer  # imported only when used: ONNX Runtime is optional

        self.charlm = CharLMScorer(model, threads)
        self.charlm_weight, self.charlm_top = weight, top
        # Merging hypotheses with equal n-gram state leaves near-identical results in
        # nbest(); without merging the top few are real alternatives worth re-ranking.
        if hasattr(self, "decoder"):
            self.decoder.merge = False

    def rescore(self, decoder: Decoder, context: str) -> None:
        """Refresh a decoder's model scores after its buffer changed (no-op without models)."""
        if self.neural is not None:
            scores = self.neural.score(context, decoder.syllables) if decoder.syllables else None
            decoder.rescore(scores, self.neural_weight, self.neural_floor)
        if self.charlm is not None and decoder.syllables:
            hyps = decoder.nbest(self.charlm_top)
            if len(hyps) > 1:
                lm = self.charlm.score(context, ["".join(words) for words, _ in hyps])
                w = self.charlm_weight
                decoder.choice = max(zip(hyps, lm), key=lambda hl: hl[0][1] + w * hl[1])[0][0]

    def set_style(self, style: str) -> None:
        """Style for this engine's own decoder and for decoders created afterwards."""
        self.decoder.set_style(style)
        self._decoder_args["style"] = style

    def new_decoder(self) -> Decoder:
        """A decoder with its own buffer, sharing this engine's dictionary and models."""
        decoder = Decoder(self.lexicon, self.lm, extra_score=self.user.bonus if self.user else None,
                          **self._decoder_args)
        decoder.merge = self.charlm is None
        return decoder

    def start(self, context: str = "") -> None:
        """Begin a composition; `context` is the text before the cursor (never modified)."""
        self.decoder.reset(context_state(context, self.lexicon, self.lm))

    def convert(self, syllables: list[str], context: str = "", nbest: int = 1) -> list[tuple[str, float]]:
        self.start(context)
        for s in syllables:
            self.decoder.push(s)
        self.rescore(self.decoder, context)
        show = self.decoder.show
        out = [("".join(show(words)), score) for words, score in self.decoder.nbest(max(nbest, self.charlm_top))]
        seen: set[str] = set()  # casual 地/得 -> 的 can make two results equal
        out = [r for r in out if not (r[0] in seen or seen.add(r[0]))]
        if self.decoder.choice is not None:  # the re-ranker's pick goes first
            pick = "".join(show(self.decoder.choice))
            out.sort(key=lambda r: r[0] != pick)
        return out[:nbest]

    def convert_keys(self, keys: str, layout: str = "dachen", **kw) -> list[tuple[str, float]]:
        return self.convert(LAYOUTS[layout].to_syllables(keys), **kw)
