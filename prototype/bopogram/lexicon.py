"""Pronunciation dictionary: bopomofo syllable sequences -> candidate words."""

from __future__ import annotations

from collections import defaultdict
from pathlib import Path

MAX_WORD_LEN = 8


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
