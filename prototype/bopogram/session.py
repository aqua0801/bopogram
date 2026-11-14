"""One typing session: keystrokes in, composition state and committed text out.

This is the layer a front end talks to. It owns the composition buffer, the
cursor, the candidate list, and the left context. Text that has been
committed is only ever appended to the context; it is never rewritten.
"""

from __future__ import annotations

from .context import context_state
from .engine import Engine
from .layouts import LAYOUTS
from .layouts.base import FINALS, INITIALS, MEDIALS, TONE_FIRST
from .lexicon import MAX_WORD_LEN

MAX_CONTEXT = 200


class Session:
    def __init__(self, engine: Engine, layout: str = "dachen"):
        self.engine = engine
        self.decoder = engine.decoder
        self.layout = LAYOUTS[layout]
        self.context = ""
        self.learn = True  # the front end turns this off for password fields
        self._clear()

    def _clear(self) -> None:
        self.initial = self.medial = self.final = ""
        self.cursor = 0  # syllable index the candidate list refers to
        self.manual: set[tuple[int, int]] = set()
        self.decoder.reset(context_state(self.context, self.engine.lexicon, self.engine.lm))

    # -- input ------------------------------------------------------------
    def set_context(self, text: str) -> None:
        """Text before the cursor, as reported by the application. Only valid while idle."""
        self.context = text[-MAX_CONTEXT:]
        if not self.decoder.syllables and not self.pending:
            self._clear()

    @property
    def pending(self) -> str:
        """Bopomofo symbols of the syllable being typed."""
        return self.initial + self.medial + self.final

    def key(self, key: str) -> bool:
        """Feed one layout key. Returns False if the key is not part of the layout."""
        sym = self.layout.keys.get(key.lower())
        if sym is None:
            return False
        if sym in INITIALS:
            self.initial = sym
        elif sym in MEDIALS:
            self.medial = sym
        elif sym in FINALS:
            self.final = sym
        else:
            body = self.pending
            if not body:
                return False
            syllable = body if sym == TONE_FIRST else ("˙" + body if sym == "˙" else body + sym)
            self.initial = self.medial = self.final = ""
            at_end = self.cursor == len(self.decoder.syllables)
            self.decoder.push(syllable)
            if at_end:
                self.cursor = len(self.decoder.syllables)
        return True

    def backspace(self) -> None:
        if self.final:
            self.final = ""
        elif self.medial:
            self.medial = ""
        elif self.initial:
            self.initial = ""
        elif self.decoder.syllables:
            self.decoder.pop()
            n = len(self.decoder.syllables)
            self.manual = {s for s in self.manual if s[1] <= n}
            self.cursor = min(self.cursor, n)

    def move(self, delta: int) -> None:
        self.cursor = max(0, min(len(self.decoder.syllables), self.cursor + delta))

    def escape(self) -> None:
        self._clear()

    # -- candidates -------------------------------------------------------
    def _segments(self) -> list[str]:
        res = self.decoder.nbest(1)
        return res[0][0] if res else []

    def _span_at_cursor(self) -> tuple[int, int] | None:
        """Start of the word under the cursor (the word before it when at the end)."""
        pos = 0
        target = min(self.cursor, len(self.decoder.syllables) - 1)
        for word in self._segments():
            if pos <= target < pos + len(word):
                return pos, pos + len(word)
            pos += len(word)
        return None

    def candidates(self) -> list[dict]:
        """Words that can start at the word under the cursor, longest first."""
        span = self._span_at_cursor()
        if span is None:
            return []
        start = span[0]
        syls = self.decoder.syllables
        out, seen = [], set()
        for length in range(min(MAX_WORD_LEN, len(syls) - start), 0, -1):
            words = self.engine.lexicon.candidates(tuple(syls[start:start + length]))
            for word in sorted(words, key=self.engine.lm.unigram, reverse=True):
                if word not in seen:
                    seen.add(word)
                    out.append({"word": word, "start": start, "length": length})
        return out

    def select(self, index: int) -> bool:
        """Pick candidate `index`; that span is fixed and the rest is decoded again."""
        cands = self.candidates()
        if not 0 <= index < len(cands):
            return False
        c = cands[index]
        span = (c["start"], c["start"] + c["length"])
        pinned = {s: w for s, w in self.decoder.pinned.items() if s[1] <= span[0] or s[0] >= span[1]}
        pinned[span] = c["word"]
        self.manual = {s for s in self.manual if s in pinned} | {span}
        self.decoder.repin(pinned)
        self.cursor = min(len(self.decoder.syllables), span[1])
        return True

    # -- output -----------------------------------------------------------
    def preedit(self) -> str:
        return "".join(self._segments()) + self.pending

    def commit(self) -> str:
        """Finish the composition and return its text."""
        words = self._segments()
        text = "".join(words)
        if text and self.learn and self.engine.user is not None:
            # Words the user picked by hand count as corrections.
            pos, picked = 0, []
            for w in words:
                picked.append((pos, pos + len(w)) in self.manual)
                pos += len(w)
            self.engine.user.learn(words, corrected=False)
            for w, was_picked in zip(words, picked):
                if was_picked:
                    self.engine.user.learn([w], corrected=True)
            self.engine.user.save()
        self.context = (self.context + text)[-MAX_CONTEXT:]
        self._clear()
        return text

    def state(self) -> dict:
        return {"preedit": self.preedit(), "pending": self.pending, "cursor": self.cursor,
                "syllables": len(self.decoder.syllables), "segments": self._segments()}
