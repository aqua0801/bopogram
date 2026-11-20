"""Pronunciation dictionary: bopomofo syllable sequences -> candidate words."""

from __future__ import annotations

from collections import defaultdict
from pathlib import Path

import math

MAX_WORD_LEN = 8
READING_FLOOR = -8.0
# Spellings accepted as input that stand for another reading (training/dict/typing_variants.tsv).
READING_ALIASES = {"˙ㄇㄛ": "˙ㄇㄜ"}


class Lexicon:
    """In-memory index over dict.tsv (word, syllables, source tag)."""

    def __init__(self, dict_tsv: str | Path):
        by_reading: dict[tuple[str, ...], list[str]] = defaultdict(list)
        readings: dict[str, list[tuple[tuple[str, ...], str]]] = defaultdict(list)
        with open(dict_tsv, encoding="utf-8") as fh:
            for line in fh:
                parts = line.rstrip("\n").split("\t")
                if len(parts) < 2:
                    continue
                word, syls = parts[0], tuple(parts[1].split())
                tag = parts[2] if len(parts) > 2 else ""
                if word not in by_reading[syls]:
                    by_reading[syls].append(word)
                readings[word].append((syls, tag))
        self._by_reading = dict(by_reading)
        self._readings = dict(readings)
        self._reading_logp: dict[tuple[str, str], float] = {}
        # The user's dictionary (userdict.py) lies on top; the base tables never change.
        # `version` goes up with every change, so decoders know their results are stale.
        self.version = 0
        self.user_bonus = 0.0
        self._user = None
        self._user_by_reading: dict[tuple[str, ...], list[str]] = {}
        self._user_entries: set[tuple[str, tuple[str, ...]]] = set()
        self._user_readings: dict[str, list[tuple[str, ...]]] = {}
        # Excluded text -> readings (None = every reading); proper prefixes for the decoder.
        self.excluded: dict[str, list] = {}
        self.excluded_lengths: list[int] = []
        self.excluded_prefixes: set[str] = set()
        self._filtered: dict[tuple[str, ...], list[str]] = {}

    # -- the user's dictionary ---------------------------------------------
    def attach(self, user, bonus: float = 0.0) -> None:
        """Lay a UserDict over the dictionary (None removes it)."""
        self._user, self.user_bonus = user, bonus
        self.refresh()

    def refresh(self) -> None:
        """Rebuild the overlay after the UserDict changed."""
        u = self._user
        self._user_by_reading, self._user_entries, self._user_readings = {}, set(), {}
        self.excluded = {}
        if u is not None:
            for word, readings in u.user.items():
                self._user_readings[word] = list(readings)
                for syls in readings:
                    self._user_entries.add((word, syls))
                    self._user_by_reading.setdefault(syls, []).append(word)
            self.excluded = {t: list(r) for t, r in u.excluded.items() if r}
        self.excluded_lengths = sorted({len(t) for t in self.excluded})
        self.excluded_prefixes = {t[:k] for t in self.excluded for k in range(1, len(t))}
        self._filtered = {}
        self.version += 1

    @property
    def is_user_active(self) -> bool:
        return bool(self._user_entries)

    def is_user(self, word: str, syllables: tuple[str, ...]) -> bool:
        return (word, syllables) in self._user_entries

    def blocked(self, text: str, syllables) -> bool:
        """Does `text` (read as `syllables`, one per character) contain excluded text?"""
        if not self.excluded:
            return False
        for n in self.excluded_lengths:
            for k in range(len(text) - n + 1):
                readings = self.excluded.get(text[k:k + n])
                if readings and (None in readings or tuple(syllables[k:k + n]) in readings):
                    return True
        return False

    def load_reading_prior(self, path: str | Path, weight: float = 1.0) -> None:
        """Table of P(syllable | character) from training/neural/reading_counts.py.

        For a character in the table, a reading scores weight * ln P, never below
        weight * READING_FLOOR (a reading the table does not list gets the floor).
        Characters not in the table, and an engine without a table, treat all
        readings alike.
        """
        probs: dict[str, dict[str, float]] = defaultdict(dict)
        with open(path, encoding="utf-8") as fh:
            for line in fh:
                parts = line.rstrip("\n").split("\t")
                if len(parts) == 3:
                    probs[parts[0]][parts[1]] = float(parts[2])
        for ch, seen in probs.items():
            for syls, _ in self._readings.get(ch, ()):
                syl = READING_ALIASES.get(syls[0], syls[0])
                p = seen.get(syl, 0.0)
                self._reading_logp[(ch, syl)] = weight * max(READING_FLOOR, math.log(p) if p > 0 else READING_FLOOR)

    def reading_logp(self, char: str, syllable: str) -> float:
        """ln P(syllable | char) for a single character; 0.0 when unknown."""
        return self._reading_logp.get((char, READING_ALIASES.get(syllable, syllable)), 0.0)

    def __contains__(self, word: str) -> bool:
        return word in self._readings or word in self._user_readings

    def words(self):
        return self._readings.keys()

    def in_base(self, word: str, syllables: tuple[str, ...] | None = None) -> bool:
        """Is the word (with this reading) in the dictionary itself, not the user's?"""
        return any(syllables is None or s == syllables for s, _ in self._readings.get(word, ()))

    def candidates(self, syllables: tuple[str, ...]) -> list[str]:
        """Words pronounced exactly as `syllables` (one syllable per character):
        the user's words first, then the dictionary's, without excluded text."""
        base = self._by_reading.get(syllables, [])
        if not self._user_by_reading and not self.excluded:
            return base
        hit = self._filtered.get(syllables)
        if hit is None:
            user = self._user_by_reading.get(syllables, [])
            words = user + [w for w in base if w not in user] if user else base
            hit = [w for w in words if not self.blocked(w, syllables)]
            if len(self._filtered) > 200_000:
                self._filtered.clear()
            self._filtered[syllables] = hit
        return hit

    def readings(self, word: str) -> list[tuple[tuple[str, ...], str]]:
        """All (syllables, source tag) pairs of a word, the user's readings included."""
        base = self._readings.get(word, [])
        user = self._user_readings.get(word)
        if not user:
            return base
        have = {s for s, _ in base}
        return base + [(s, "user") for s in user if s not in have]
