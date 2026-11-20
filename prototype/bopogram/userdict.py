"""The user's own dictionary: words added by hand, and text excluded from the results.

Two local files, both in the dict.tsv format (UTF-8, Tab separated, one
syllable per character with syllables separated by one space; blank lines and
lines starting with "#" are skipped):

    userdict.tsv   word <Tab> reading <Tab> user
    excluded.tsv   text <Tab> reading      (only that reading)
                   text                    (every reading)

Only the engine writes them (front ends go through the protocol). Each change
is written at once, through a temporary file, so a crash never leaves half a
file. Lines that cannot be read are skipped and reported by line number only,
never by content. See crates/docs/userdict-handoff.md for the full rules.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

from .layouts.base import FINALS, INITIALS, MEDIALS
from .lexicon import MAX_WORD_LEN

SYLLABIC_INITIALS = set("ㄓㄔㄕㄖㄗㄘㄙ")  # the initials that are a syllable on their own
ANY = None  # an exclusion that covers every reading of the text
USER_TAG = "user"
_TONE_MARKS = set("ˊˇˋ")

Reading = tuple[str, ...]


class DictError(ValueError):
    """A request the dictionary cannot carry out; `reason` is for the front end."""

    def __init__(self, reason: str, message: str):
        super().__init__(message)
        self.reason = reason


def valid_syllable(syl: str) -> bool:
    """Structurally valid bopomofo: [˙] [initial] [medial] [final] [tone mark]."""
    body = syl[1:] if syl.startswith("˙") else syl
    if body and body[-1] in _TONE_MARKS:
        if syl.startswith("˙"):
            return False
        body = body[:-1]
    if not body:
        return False
    if len(body) == 1 and body in INITIALS and body not in SYLLABIC_INITIALS:
        return False
    k = 0
    for group in (INITIALS, MEDIALS, FINALS):
        if k < len(body) and body[k] in group:
            k += 1
    return k == len(body)


def parse_reading(text: str, reading: str | list | tuple) -> Reading:
    """Check `reading` against `text`; DictError with the reason if it does not fit."""
    syls = tuple(reading.split()) if isinstance(reading, str) else tuple(reading)
    if not text:
        raise DictError("length_mismatch", "empty text")
    if len(text) > MAX_WORD_LEN:
        raise DictError("too_long", f"at most {MAX_WORD_LEN} characters")
    if not syls or not all(valid_syllable(s) for s in syls):
        raise DictError("bad_reading", "the reading has an invalid syllable")
    if len(syls) != len(text):
        raise DictError("length_mismatch", "one syllable per character is required")
    return syls


def check_text(text: str) -> None:
    if not text:
        raise DictError("length_mismatch", "empty text")
    if len(text) > MAX_WORD_LEN:
        raise DictError("too_long", f"at most {MAX_WORD_LEN} characters")


def read_rows(path: Path | None):
    """(line number, fields) of a dict.tsv-style file; a missing file is empty."""
    if path is None or not path.exists():
        return
    with open(path, encoding="utf-8-sig") as fh:
        for n, line in enumerate(fh, 1):
            line = line.rstrip("\r\n")
            if not line.strip() or line.startswith("#"):
                continue
            yield n, line.split("\t")


def _skip(path: Path, n: int) -> None:
    print(f"{path.name}: line {n} skipped", file=sys.stderr)


class UserDict:
    def __init__(self, user_path: str | Path | None, excluded_path: str | Path | None):
        self.user_path = Path(user_path) if user_path else None
        self.excluded_path = Path(excluded_path) if excluded_path else None
        self.reload()

    def reload(self) -> None:
        # Insertion-ordered: the files are written back in the order they were read.
        self.user: dict[str, list[Reading]] = {}
        self.excluded: dict[str, list[Reading | None]] = {}
        for n, f in read_rows(self.user_path):
            try:
                if len(f) < 2:
                    raise DictError("bad_reading", "")
                syls = parse_reading(f[0], f[1])
            except DictError:
                _skip(self.user_path, n)
                continue
            if syls not in self.user.setdefault(f[0], []):
                self.user[f[0]].append(syls)
        for n, f in read_rows(self.excluded_path):
            try:
                if len(f) == 1 or not f[1].strip():
                    check_text(f[0])
                    syls = ANY
                else:
                    syls = parse_reading(f[0], f[1])
            except DictError:
                _skip(self.excluded_path, n)
                continue
            if syls not in self.excluded.setdefault(f[0], []):
                self.excluded[f[0]].append(syls)

    # -- changes (each one is written at once) ------------------------------
    def _need(self, path: Path | None) -> None:
        if path is None:
            raise DictError("disabled", "this feature is turned off in config.json")

    def add(self, word: str, syls: Reading) -> bool:
        """False if the entry was there already."""
        self._need(self.user_path)
        if syls in self.user.get(word, ()):
            return False
        new = self._copy(self.user)
        new.setdefault(word, []).append(syls)
        self._set_user(new)
        return True

    def remove_user(self, word: str, syls: Reading | None) -> bool:
        """Remove a user word (one reading, or all of them)."""
        have = self.user.get(word)
        if not have or (syls is not None and syls not in have):
            return False
        self._need(self.user_path)
        new = self._copy(self.user)
        keep = [] if syls is None else [s for s in have if s != syls]
        if keep:
            new[word] = keep
        else:
            del new[word]
        self._set_user(new)
        return True

    def exclude(self, text: str, syls: Reading | None) -> None:
        self._need(self.excluded_path)
        have = self.excluded.get(text, [])
        if ANY in have or syls in have:
            return
        new = self._copy(self.excluded)
        # The text without a reading covers the single readings.
        new[text] = [ANY] if syls is ANY else have + [syls]
        self._set_excluded(new)

    def restore(self, text: str, syls: Reading | None) -> bool:
        """Lift an exclusion: the given reading (and an all-readings entry), or all of them."""
        have = self.excluded.get(text)
        if not have:
            return False
        keep = [] if syls is None else [s for s in have if s not in (syls, ANY)]
        if len(keep) == len(have):
            return False
        self._need(self.excluded_path)
        new = self._copy(self.excluded)
        if keep:
            new[text] = keep
        else:
            del new[text]
        self._set_excluded(new)
        return True

    def entries(self) -> list[dict]:
        out = [{"word": w, "reading": " ".join(s), "kind": "user"} for w, ss in self.user.items() for s in ss]
        out += [{"word": t, "reading": "" if s is ANY else " ".join(s), "kind": "excluded"}
                for t, ss in self.excluded.items() for s in ss]
        return out

    # -- files: a change takes effect only once it is on disk ------------------
    @staticmethod
    def _copy(table: dict) -> dict:
        return {k: list(v) for k, v in table.items()}

    @staticmethod
    def _write(path: Path, lines: list[str]) -> None:
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            tmp = path.with_suffix(path.suffix + ".tmp")
            with open(tmp, "w", encoding="utf-8", newline="\n") as fh:
                fh.write("".join(line + "\n" for line in lines))
            os.replace(tmp, path)
        except OSError as err:
            raise DictError("write_failed", f"cannot write {path.name}: {err.strerror or err}") from err

    def _set_user(self, new: dict) -> None:
        self._write(self.user_path, [f"{w}\t{' '.join(s)}\t{USER_TAG}" for w, ss in new.items() for s in ss])
        self.user = new

    def _set_excluded(self, new: dict) -> None:
        self._write(self.excluded_path, [t if s is ANY else f"{t}\t{' '.join(s)}"
                                         for t, ss in new.items() for s in ss])
        self.excluded = new
