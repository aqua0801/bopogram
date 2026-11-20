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
import time
from pathlib import Path


SAVE_INTERVAL = 30.0


class UserModel:
    def __init__(self, path: str | Path | None = None, weight: float = 2.0,
                 unigram_weight: float = 0.3, correction_step: int = 3, accepted_step: int = 0):
        self.path = Path(path) if path else None
        self.weight = weight
        self.unigram_weight = unigram_weight    # share of the word count in the bonus
        self.correction_step = correction_step  # a word picked by hand counts this many times
        # A word taken as the engine chose it. 0 by default: learning accepted text only
        # reinforces what the models already prefer and makes the bonus favour cutting a
        # sentence into many familiar words. Over the dev set (eval/tools/simulate_learning.py)
        # learning everything lowered first-try accuracy 90.3% -> 89.9%; learning only
        # corrections raised it to 90.5% and a second pass over the same text to 96.0%.
        self.accepted_step = accepted_step
        self.unigrams: dict[str, int] = {}
        self.bigrams: dict[str, int] = {}
        self._saved_at = 0.0
        self._dirty = False
        if self.path and self.path.exists():
            data = json.loads(self.path.read_text(encoding="utf-8"))
            self.unigrams = data.get("unigrams", {})
            self.bigrams = data.get("bigrams", {})

    def learn(self, words: list[str], picked: set[int] | frozenset = frozenset()) -> None:
        """Record a committed word sequence; `picked` are the indices of the words the
        user chose by hand. A pair counts as a correction when either word was picked."""
        prev = None
        for k, w in enumerate(words):
            step = self.correction_step if k in picked else self.accepted_step
            if step:
                self.unigrams[w] = self.unigrams.get(w, 0) + step
                self._dirty = True
            if prev is not None:
                pstep = self.correction_step if (k in picked or k - 1 in picked) else self.accepted_step
                if pstep:
                    key = prev + "\t" + w
                    self.bigrams[key] = self.bigrams.get(key, 0) + pstep
                    self._dirty = True
            prev = w

    def bonus(self, prev: str | None, word: str) -> float:
        """Added log score for choosing `word` after `prev`."""
        b = self.bigrams.get(prev + "\t" + word, 0) if prev else 0
        u = self.unigrams.get(word, 0)
        if not b and not u:
            return 0.0
        return self.weight * (math.log1p(b) + self.unigram_weight * math.log1p(u))

    def forget(self, text: str) -> int:
        """Drop every word and word pair whose text contains `text` (used when the user
        removes or excludes it). Returns how many entries went; the file is saved."""
        drop_u = [w for w in self.unigrams if text in w]
        drop_b = [k for k in self.bigrams if text in k.replace("\t", "")]
        for w in drop_u:
            del self.unigrams[w]
        for k in drop_b:
            del self.bigrams[k]
        if drop_u or drop_b:
            self._dirty = True
            self.save(force=True)
        return len(drop_u) + len(drop_b)

    def save(self, force: bool = False) -> None:
        """Write the file, at most once every SAVE_INTERVAL seconds unless `force`:
        rewriting it on every commit would make each commit slower as it grows."""
        if not self.path or not self._dirty:
            return
        if not force and time.monotonic() - self._saved_at < SAVE_INTERVAL:
            return
        self._saved_at, self._dirty = time.monotonic(), False
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix(".tmp")
        tmp.write_text(json.dumps({"unigrams": self.unigrams, "bigrams": self.bigrams},
                                  ensure_ascii=False), encoding="utf-8")
        os.replace(tmp, self.path)
