"""Personalisation: a small user n-gram learned from committed text.

Only word unigram and bigram counts are stored, in a local JSON file. The
model contributes a bonus to the decoder score; it never changes the base
model or any character frequency. Nothing here touches the network, and the
front end must not call `learn` for password fields.
"""

from __future__ import annotations

import json
import math
import os
from pathlib import Path


class UserModel:
    def __init__(self, path: str | Path | None = None, weight: float = 1.0):
        self.path = Path(path) if path else None
        self.weight = weight
        self.unigrams: dict[str, int] = {}
        self.bigrams: dict[str, int] = {}
        if self.path and self.path.exists():
            data = json.loads(self.path.read_text(encoding="utf-8"))
            self.unigrams = data.get("unigrams", {})
            self.bigrams = data.get("bigrams", {})

    def learn(self, words: list[str], corrected: bool = False) -> None:
        """Record a committed word sequence. A manual correction counts more."""
        step = 3 if corrected else 1
        prev = None
        for w in words:
            self.unigrams[w] = self.unigrams.get(w, 0) + step
            if prev is not None:
                key = prev + "\t" + w
                self.bigrams[key] = self.bigrams.get(key, 0) + step
            prev = w

    def bonus(self, prev: str | None, word: str) -> float:
        """Added log score for choosing `word` after `prev`."""
        b = self.bigrams.get(prev + "\t" + word, 0) if prev else 0
        u = self.unigrams.get(word, 0)
        if not b and not u:
            return 0.0
        return self.weight * (math.log1p(b) + 0.3 * math.log1p(u))

    def save(self) -> None:
        if not self.path:
            return
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix(".tmp")
        tmp.write_text(json.dumps({"unigrams": self.unigrams, "bigrams": self.bigrams},
                                  ensure_ascii=False), encoding="utf-8")
        os.replace(tmp, self.path)
