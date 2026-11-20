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

    def _sync(self) -> None:
        """If the user's dictionary changed, decode the buffer again with it. Called
        before anything that changes or commits the buffer, so that a change shows on
        the next key and never moves the text under the user's eyes."""
        if self.decoder.refresh():
            self._rescore()

    def _insert(self, item: str) -> None:
        self._sync()
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
            self._sync()
            self.decoder.delete(self.cursor - 1)
            self.cursor -= 1
            self._rescore()

    def delete(self) -> None:
        """Remove the item after the cursor (the Delete key)."""
        if not self.pending and self.cursor < self._size:
            self._sync()
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
        self._rescore()
        return True

    # -- candidates -------------------------------------------------------
    def _segments(self) -> list[str]:
        return self.decoder.best_words()

    def _items(self) -> list[tuple[str, int, int]]:
        """(text, word start, word end) for every buffer item; see Decoder.best_items."""
        return self.decoder.best_items()

    def _span_at_cursor(self) -> tuple[int, int] | None:
        """The word the candidate list refers to: the one holding the character
        after the cursor (the last one when the cursor is at the end). With
        `candidate_side` "before" it is the character before the cursor instead."""
        if not self._size:
            return None
        target = self.cursor if self.candidate_side == "after" else self.cursor - 1
        target = max(0, min(self._size - 1, target))
        items = self._items()
        if target >= len(items):
            return None
        _, start, end = items[target]
        if self.candidate_side == "after" and self.cursor < self._size:
            # Inside the buffer the list starts at the character after the cursor, even in
            # the middle of a word: with 跑道 and the cursor before 道 it offers 道, not 跑.
            start = target
        return start, end

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
        lexicon, unigram = self.engine.lexicon, self.engine.lm.unigram
        for length in range(min(MAX_WORD_LEN, len(syls) - start), 0, -1):
            key = tuple(syls[start:start + length])
            words = lexicon.candidates(key)
            if length == 1:  # single characters: frequent under THIS reading first
                score = lambda w: unigram(w) + lexicon.reading_logp(w, key[0])
            else:
                score = unigram
            # The user's own words come first.
            rank = lambda w: (lexicon.is_user(w, key), score(w))
            for word in sorted(words, key=rank, reverse=True):
                if word not in seen:
                    seen.add(word)
                    out.append({"word": word, "start": start, "length": length})
        return out

    def select(self, index: int) -> bool:
        """Pick candidate `index`; that span is fixed and the rest is decoded again."""
        cands = self.candidates()
        if not 0 <= index < len(cands):
            return False
        self._sync()
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
        self._sync()
        span = self._span_at_cursor()
        if span is None:
            return False
        current = "".join(t for t, _, _ in self._items()[span[0]:span[1]])
        cands = self.candidates()
        same = [i for i, c in enumerate(cands) if c["length"] == span[1] - span[0]]
        if len(same) < 2:
            return False
        at = next((k for k, i in enumerate(same) if cands[i]["word"] == current), -1)
        return self.select(same[(at + delta) % len(same)])

    # -- output -----------------------------------------------------------
    def preedit(self) -> str:
        texts = [t for t, _, _ in self._items()]
        return "".join(texts[:self.cursor]) + self.pending + "".join(texts[self.cursor:])

    def _learn(self, count: int) -> None:
        """Record the words covering the first `count` items; runs are cut at literals
        and unconverted syllables, and words picked by hand count as corrections."""
        user = self.engine.user
        if not self.learn or user is None:
            return
        buf, run, picked, last = self.decoder.syllables, [], [], None

        def flush() -> None:
            if run:
                user.learn(list(run), set(picked))
            run.clear()
            picked.clear()

        items = self._items()[:count]
        for k, (text, a, b) in enumerate(items):
            if (a, b) == last:
                continue
            last = (a, b)
            word = "".join(t for t, _, _ in items[a:b])
            if (b > count or is_literal(buf[a]) or (b - a == 1 and len(text) > 1)
                    or word not in self.engine.lexicon or self.engine.lexicon.blocked(word, buf[a:b])):
                flush()
                continue
            run.append(word)
            if (a, b) in self.decoder.pinned:
                picked.append(len(run) - 1)
        flush()
        user.save()

    def _record(self, count: int, auto: bool) -> None:
        """Write the first `count` items to the typing log (if enabled and learning is on)."""
        log = getattr(self.engine, "typing_log", None)
        if log is None or not self.learn or not count:
            return
        dec = self.decoder
        texts = [t for t, _, _ in self._items()]
        first = texts
        if dec.pinned:  # what the engine would have chosen without the user's picks
            plain = self.engine.new_decoder()
            plain.style = dec.style
            plain.reset(dec.nodes[0][0].state)
            plain.set_position_scores(dec.position_scores, dec.position_weight, dec.position_floor)
            for item in dec.syllables:
                plain.push(item)
            first = [t for t, _, _ in plain.best_items()]
        picked = [(a, b - a, w) for (a, b), w in sorted(dec.pinned.items()) if b <= count]
        log.write(dec.syllables[:count], "".join(first[:count]), "".join(texts[:count]), picked,
                  self.context, dec.style, auto)

    def _limit(self) -> None:
        """Commit the first words when the buffer is too long. Committed text is final."""
        if self._size <= MAX_BUFFER:
            return
        items, count = self._items(), 0
        while count < len(items) and self._size - count > MAX_BUFFER and items[count][2] <= self.cursor:
            count = items[count][2]  # whole words only, and never past the cursor
        if not count:
            return
        self._learn(count)
        self._record(count, auto=True)
        text = "".join(t for t, _, _ in items[:count])
        self._flushed += text
        self.context = (self.context + text)[-MAX_CONTEXT:]
        self.decoder.drop_front(count, context_state(self.context, self.engine.lexicon, self.engine.lm))
        self.cursor -= count
        self._rescore()

    def commit(self) -> str:
        """Finish the composition and return its text (a syllable in progress is dropped)."""
        self._sync()
        text = "".join(self._segments())
        if text:
            self._learn(self._size)
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
                "caret": len("".join(t for t, _, _ in self._items()[:self.cursor])) + len(self.pending),
                "syllables": self._size,
                "segments": self._segments(), "style": self.decoder.style}
