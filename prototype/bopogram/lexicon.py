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
        return word in self._readings

    def words(self):
        return self._readings.keys()

    def candidates(self, syllables: tuple[str, ...]) -> list[str]:
        """Words pronounced exactly as `syllables` (one syllable per character)."""
        return self._by_reading.get(syllables, [])

    def readings(self, word: str) -> list[tuple[tuple[str, ...], str]]:
        """All (syllables, source tag) pairs of a word."""
        return self._readings.get(word, [])
