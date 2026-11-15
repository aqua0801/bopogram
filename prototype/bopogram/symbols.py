"""Full-width punctuation for symbol keys typed in Chinese mode.

The front end sends the plain ASCII character of the key (with Shift applied,
e.g. "<" for Shift+comma); the engine inserts the full-width form into the
composition buffer. The mapping follows the usual Traditional Chinese
convention: every ASCII symbol becomes its full-width form, except the few
whose customary Chinese punctuation is a different character.
"""

from __future__ import annotations

_SPECIAL = {
    ",": "，", ".": "。", "<": "，", ">": "。", "'": "、", "\\": "、",
    "[": "「", "]": "」", "{": "『", "}": "』", "\"": "；", "^": "……", "_": "──", " ": " ",
}


def full_width(key: str) -> str | None:
    """Full-width text for one ASCII symbol key, or None if `key` is not a symbol."""
    if len(key) != 1 or key.isalnum() or not (" " <= key <= "~"):
        return None
    hit = _SPECIAL.get(key)
    return hit if hit is not None else chr(ord(key) + 0xFEE0)
