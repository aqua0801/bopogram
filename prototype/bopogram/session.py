"""One typing session: keystrokes in, composition state and committed text out.

This is the layer a front end talks to. It owns the composition buffer, the
cursor, the candidate list, and the left context. Text that has been
committed is only ever appended to the context; it is never rewritten.

Editing rules (they follow the behaviour Microsoft documents for its
Bopomofo IME, so that users of that IME need not relearn anything):

  * Bopomofo keys build a syllable at the cursor; a tone key (Space for the
    first tone) completes it and inserts it at the cursor.
  * Space with no syllable in progress inserts a space into the buffer.
  * Left / Right / Home / End move the cursor inside the buffer. Backspace
    removes the item before the cursor, Delete the one after it. While a
    syllable is in progress, Backspace removes its last symbol.
  * Symbols and other text can be inserted at the cursor; they stay as typed.
    A symbol can be changed to a related one from the candidate list.
  * Down opens the candidates of the word after the cursor (the last word when
    the cursor is at the end); Up changes that word
    to its next candidate without opening the list.
  * Escape discards the syllable in progress, or else the whole buffer.
  * When the buffer grows past MAX_BUFFER, its first words are committed.
"""

from __future__ import annotations

from .context import context_state
from .decoder import is_literal, literal
from .engine import Engine
from .layouts import LAYOUTS
from .layouts.base import FINALS, INITIALS, MEDIALS, TONE_FIRST
from .lexicon import MAX_WORD_LEN
from .symbols import alternatives, full_width

MAX_CONTEXT = 200
MAX_BUFFER = 30  # items kept in the composition buffer before the front is committed


class Session:
    def __init__(self, engine: Engine, layout: str = "dachen"):
        self.engine = engine
        # Each session has its own composition buffer; dictionary and models are shared.
        self.decoder = engine.new_decoder()
        self.layout = LAYOUTS[layout]
        self.context = ""
        self.learn = True  # the front end turns this off for password fields
        # Which character the candidate list refers to: the one "after" the cursor (as in
        # Microsoft Bopomofo; at the end of the buffer it is the last character) or "before" it.
        self.candidate_side = "after"
        self._flushed = ""  # text committed automatically since the last state() call
        self._clear()

    def _clear(self) -> None:
        self.initial = self.medial = self.final = ""
        self.cursor = 0  # number of buffer items before the cursor
        self.decoder.set_position_scores(None)
        self.decoder.reset(context_state(self.context, self.engine.lexicon, self.engine.lm))

    @property
    def _size(self) -> int:
        return len(self.decoder.syllables)

    # -- input ------------------------------------------------------------
    def set_context(self, text: str) -> None:
        """Text before the cursor, as reported by the application. Only valid while idle."""
        self.context = text[-MAX_CONTEXT:]
        if not self._size and not self.pending:
            self._clear()

    @property
    def pending(self) -> str:
        """Bopomofo symbols of the syllable being typed."""
        return self.initial + self.medial + self.final

    def _rescore(self) -> None:
        rescore = getattr(self.engine, "rescore", None)
        if rescore is not None:
            rescore(self.decoder, self.context)

    def _insert(self, item: str) -> None:
        self.decoder.insert(self.cursor, item)
        self.cursor += 1
        self._rescore()
        self._limit()

    def key(self, key: str) -> bool:
        """Feed one layout key. Returns False if the key is not handled
        (not part of the layout, or Space with nothing being composed)."""
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
                if sym == TONE_FIRST and self._size:
                    self._insert(literal(" "))  # Space is a space when no syllable is in progress
                    return True
                return False
            syllable = body if sym == TONE_FIRST else ("˙" + body if sym == "˙" else body + sym)
            self.initial = self.medial = self.final = ""
            self._insert(syllable)
        return True

    def text(self, text: str) -> bool:
        """Insert characters that are not converted (letters, digits, symbols)."""
        if self.pending or not text:
            return False
        for ch in text:
            self._insert(literal(ch))
        return True

    def symbol(self, key: str) -> bool:
        """Insert the full-width form of an ASCII symbol key."""
        wide = full_width(key)
        return wide is not None and self.text(wide)

    def backspace(self) -> None:
        if self.final:
            self.final = ""
        elif self.medial:
            self.medial = ""
        elif self.initial:
            self.initial = ""
        elif self.cursor > 0:
            self.decoder.delete(self.cursor - 1)
            self.cursor -= 1
            self._rescore()

    def delete(self) -> None:
        """Remove the item after the cursor (the Delete key)."""
        if not self.pending and self.cursor < self._size:
            self.decoder.delete(self.cursor)
            self._rescore()

    def move(self, delta: int) -> None:
        if not self.pending:
            self.cursor = max(0, min(self._size, self.cursor + delta))

    def move_to(self, end: bool) -> None:
        """Home (end=False) or End (end=True)."""
        if not self.pending:
            self.cursor = self._size if end else 0

    def escape(self) -> None:
        if self.pending:
            self.initial = self.medial = self.final = ""
        else:
            self._clear()

    def set_style(self, style: str) -> bool:
        """"casual" or "formal" (see Decoder.style); the buffer is decoded again."""
        try:
            self.decoder.set_style(style)
        except ValueError:
            return False
        return True

    # -- candidates -------------------------------------------------------
    def _segments(self) -> list[str]:
        res = self.decoder.nbest(1)
        return res[0][0] if res else []

    def _span_at_cursor(self) -> tuple[int, int] | None:
        """The word the candidate list refers to: the one holding the character
        after the cursor (the last one when the cursor is at the end). With
        `candidate_side` "before" it is the character before the cursor instead."""
        if not self._size:
            return None
        target = self.cursor if self.candidate_side == "after" else self.cursor - 1
        target = max(0, min(self._size - 1, target))
        pos = 0
        for word in self._segments():
            if pos <= target < pos + len(word):
                return pos, pos + len(word)
            pos += len(word)
        return None

    def candidates(self) -> list[dict]:
        """Words that can start where the word at the cursor starts, longest first."""
        span = self._span_at_cursor()
        if span is None:
            return []
        start = span[0]
        syls = self.decoder.syllables
        if is_literal(syls[start]):  # a symbol can be changed to a related symbol
            return [{"word": alt, "start": start, "length": 1} for alt in alternatives(syls[start][1:])]
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
        if is_literal(self.decoder.syllables[c["start"]]):
            self.decoder.delete(c["start"])
            self.decoder.insert(c["start"], literal(c["word"]))
            self._rescore()
            return True
        span = (c["start"], c["start"] + c["length"])
        pinned = {s: w for s, w in self.decoder.pinned.items() if s[1] <= span[0] or s[0] >= span[1]}
        pinned[span] = c["word"]
        self.decoder.repin(pinned)
        self.cursor = max(self.cursor, min(self._size, span[1]))
        return True

    def cycle(self, delta: int = 1) -> bool:
        """Change the word at the cursor to its next (or previous) candidate of the
        same length, without opening the candidate list."""
        span = self._span_at_cursor()
        if span is None:
            return False
        current = "".join(self._segments())[span[0]:span[1]]
        cands = self.candidates()
        same = [i for i, c in enumerate(cands) if c["length"] == span[1] - span[0]]
        if len(same) < 2:
            return False
        at = next((k for k, i in enumerate(same) if cands[i]["word"] == current), -1)
        return self.select(same[(at + delta) % len(same)])

    # -- output -----------------------------------------------------------
    def preedit(self) -> str:
        text = "".join(self._segments())
        return text[:self.cursor] + self.pending + text[self.cursor:]

    def _learn(self, words: list[str], first_pos: int) -> None:
        """Record committed words; runs are cut at literals, picked words count as corrections."""
        user = self.engine.user
        if not self.learn or user is None:
            return
        items, pos, run, picked = self.decoder.syllables, first_pos, [], []
        for w in words + [None]:
            if w is None or is_literal(items[pos]):
                if run:
                    user.learn(run, corrected=False)
                    for word in picked:
                        user.learn([word], corrected=True)
                run, picked = [], []
            else:
                run.append(w)
                if (pos, pos + len(w)) in self.decoder.pinned:
                    picked.append(w)
            if w is not None:
                pos += len(w)
        user.save()

    def _record(self, count: int, auto: bool) -> None:
        """Write the first `count` items to the typing log (if enabled and learning is on)."""
        log = getattr(self.engine, "typing_log", None)
        if log is None or not self.learn or not count:
            return
        dec = self.decoder
        text = "".join(self._segments())
        first = text
        if dec.pinned:  # what the engine would have chosen without the user's picks
            plain = self.engine.new_decoder()
            plain.style = dec.style
            plain.reset(dec.nodes[0][0].state)
            plain.set_position_scores(dec.position_scores, dec.position_weight, dec.position_floor)
            for item in dec.syllables:
                plain.push(item)
            first = plain.best_text()
        picked = [(a, b - a, w) for (a, b), w in sorted(dec.pinned.items()) if b <= count]
        log.write(dec.syllables[:count], first[:count], text[:count], picked, self.context, dec.style, auto)

    def _limit(self) -> None:
        """Commit the first words when the buffer is too long. Committed text is final."""
        if self._size <= MAX_BUFFER:
            return
        words, count, taken = self._segments(), 0, []
        for w in words:
            if self._size - count <= MAX_BUFFER or count + len(w) > self.cursor:
                break
            taken.append(w)
            count += len(w)
        if not taken:
            return
        self._learn(taken, 0)
        self._record(count, auto=True)
        text = "".join(taken)
        self._flushed += text
        self.context = (self.context + text)[-MAX_CONTEXT:]
        self.decoder.drop_front(count, context_state(self.context, self.engine.lexicon, self.engine.lm))
        self.cursor -= count
        self._rescore()

    def commit(self) -> str:
        """Finish the composition and return its text (a syllable in progress is dropped)."""
        words = self._segments()
        text = "".join(words)
        if text:
            self._learn(words, 0)
            self._record(self._size, auto=False)
        self.context = (self.context + text)[-MAX_CONTEXT:]
        self._clear()
        return text

    def take_flushed(self) -> str:
        """Text committed automatically since the last call (see MAX_BUFFER)."""
        text, self._flushed = self._flushed, ""
        return text

    def state(self) -> dict:
        return {"preedit": self.preedit(), "pending": self.pending, "cursor": self.cursor,
                "caret": self.cursor + len(self.pending), "syllables": self._size,
                "segments": self._segments(), "style": self.decoder.style}
