"""Full-width punctuation for symbol keys typed with Ctrl (or after `) in Chinese mode.

The front end sends the plain ASCII character of the key (with Shift applied
when Shift is held, e.g. "?" for Ctrl+Shift+/); the engine inserts the
full-width form into the composition buffer. Shift + symbol alone is not
handled here: as in Microsoft Bopomofo it types the half-width character, which
the front end inserts by itself.

Every ASCII symbol becomes its full-width form (? -> ？, : -> ：, ; -> ；),
except the keys whose Chinese punctuation is a different character. A symbol
in the buffer can be changed to a related one from the candidate list
(e.g. 【 -> 「), see `alternatives`.
"""

from __future__ import annotations

_SPECIAL = {",": "，", ".": "。", "'": "、", "[": "【", "]": "】", " ": " "}

# Symbols offered in place of one another, in the order shown.
_GROUPS = [
    "【「『〔［｛《〈（",
    "】」』〕］｝》〉）",
    "，、；：",
    "。．…？！",
    "＂“”〝〞",
    "＇‘’",
    "－—～＿",
]


def full_width(key: str) -> str | None:
    """Full-width text for one ASCII symbol key, or None if `key` is not a symbol."""
    if len(key) != 1 or key.isalnum() or not (" " <= key <= "~"):
        return None
    hit = _SPECIAL.get(key)
    return hit if hit is not None else chr(ord(key) + 0xFEE0)


def alternatives(char: str) -> list[str]:
    """The group of related symbols `char` belongs to, in a fixed order; [] if none."""
    for group in _GROUPS:
        if char in group:
            return list(group)
    return []
