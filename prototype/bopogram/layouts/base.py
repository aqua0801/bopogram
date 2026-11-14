"""Shared machinery for bopomofo keyboard layouts."""

from __future__ import annotations

from dataclasses import dataclass

INITIALS = set("ㄅㄆㄇㄈㄉㄊㄋㄌㄍㄎㄏㄐㄑㄒㄓㄔㄕㄖㄗㄘㄙ")
MEDIALS = set("ㄧㄨㄩ")
FINALS = set("ㄚㄛㄜㄝㄞㄟㄠㄡㄢㄣㄤㄥㄦ")
TONE_FIRST = " "          # first tone is typed with the space bar and written unmarked
TONES = {"ˊ", "ˇ", "ˋ", "˙"}


class LayoutError(ValueError):
    """Raised when a key sequence is not a valid series of syllables."""


@dataclass(frozen=True)
class Layout:
    name: str
    keys: dict[str, str]  # key -> bopomofo symbol, tone mark, or TONE_FIRST

    def to_syllables(self, keystrokes: str) -> list[str]:
        """Convert keystrokes into syllables in dictionary notation.

        Every syllable must end with a tone key. Symbols may be typed in any
        order within a syllable; a later key of the same class replaces the
        earlier one (as on a real bopomofo keyboard). The neutral tone is
        written with a leading "˙", the first tone unmarked.
        """
        out: list[str] = []
        initial = medial = final = ""
        for key in keystrokes:
            sym = self.keys.get(key.lower())
            if sym is None:
                raise LayoutError(f"key {key!r} is not part of the {self.name} layout")
            if sym in INITIALS:
                initial = sym
            elif sym in MEDIALS:
                medial = sym
            elif sym in FINALS:
                final = sym
            else:  # tone key ends the syllable
                body = initial + medial + final
                if not body:
                    raise LayoutError("tone key without a syllable")
                if sym == TONE_FIRST:
                    out.append(body)
                elif sym == "˙":
                    out.append("˙" + body)
                else:
                    out.append(body + sym)
                initial = medial = final = ""
        if initial or medial or final:
            raise LayoutError("last syllable has no tone key (use space for the first tone)")
        return out

    def to_keys(self, syllables: list[str]) -> str:
        """Inverse of to_syllables; used to generate test input."""
        rev = {v: k for k, v in self.keys.items()}
        out = []
        for syl in syllables:
            tone = TONE_FIRST
            body = syl
            if syl.startswith("˙"):
                tone, body = "˙", syl[1:]
            elif syl[-1] in TONES:
                tone, body = syl[-1], syl[:-1]
            out.append("".join(rev[ch] for ch in body) + rev[tone])
        return "".join(out)
