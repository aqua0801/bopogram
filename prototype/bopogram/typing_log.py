"""Optional local record of what was typed, for improving the engine.

Off unless the engine config names a file ("typing_log"). Each committed
composition appends one JSON line:

    {"v": 1, "t": "2026-11-15T22:40", "items": ["ㄨㄛˇ", "ㄗㄞˋ", "=，"],
     "first": "我在，", "text": "我再，", "picked": [[1, 1, "再"]],
     "context": "...", "style": "casual", "auto": false}

    items    what was typed: syllables, and literals as "=" + character
    first    what the engine chose by itself for these items
    text     what was committed
    picked   [start, length, word] for every word the user chose by hand
    context  the 40 characters before the composition
    auto     true when the engine committed the words because the buffer was full

Safeguards:
  * Nothing is written while learning is off (the front end turns it off for
    password fields), and nothing is ever sent anywhere: this module only
    appends to one local file.
  * Letters and digits are never stored. In the items, the texts and the
    context each of them is replaced by "＊", so account names, numbers and
    codes typed in between cannot end up in the file.
  * The file stops growing at MAX_BYTES.
  * The file is raw material only. Nothing reaches training or the test set
    unless it is approved record by record in tools/review_log.py.
"""

from __future__ import annotations

import json
import re
import time
from pathlib import Path

MAX_BYTES = 20_000_000
MASK = "＊"
CONTEXT_CHARS = 40
_RE_ALNUM = re.compile(r"[0-9A-Za-z０-９Ａ-Ｚａ-ｚ]")


def scrub(text: str) -> str:
    """Replace every letter and digit by MASK (length is preserved)."""
    return _RE_ALNUM.sub(MASK, text)


class TypingLog:
    def __init__(self, path: str | Path, max_bytes: int = MAX_BYTES):
        self.path = Path(path)
        self.max_bytes = max_bytes

    def write(self, items: list[str], first: str, text: str, picked: list[tuple[int, int, str]],
              context: str, style: str, auto: bool) -> bool:
        """Append one record. Returns False when nothing was written."""
        if not any(not item.startswith("=") for item in items):
            return False  # no bopomofo at all: nothing to learn from
        record = {
            "v": 1, "t": time.strftime("%Y-%m-%dT%H:%M"),
            "items": [scrub(item) for item in items],
            "first": scrub(first), "text": scrub(text),
            "picked": [[a, n, scrub(w)] for a, n, w in picked],
            "context": scrub(context[-CONTEXT_CHARS:]), "style": style, "auto": auto,
        }
        try:
            if self.path.exists() and self.path.stat().st_size >= self.max_bytes:
                return False
            self.path.parent.mkdir(parents=True, exist_ok=True)
            with open(self.path, "a", encoding="utf-8") as fh:
                fh.write(json.dumps(record, ensure_ascii=False) + "\n")
        except OSError:
            return False  # logging must never break typing
        return True
