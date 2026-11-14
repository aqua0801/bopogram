"""Left context: text already committed before the composition buffer.

The context is read-only. It is segmented into dictionary words and fed to
the language model so that the first words of the composition are scored in
context; committed text is never rewritten.
"""

from __future__ import annotations

import math
import re

from .lexicon import MAX_WORD_LEN, Lexicon
from .lm import PUNCT, MixtureLM, State

_RE_PIECE = re.compile(r"([一-鿿㐀-䶿]+)|([A-Za-z0-9][A-Za-z0-9.%+\-]*)|(\S)")
_RE_SENTENCE_END = re.compile(r"[。！？!?\n]")
MAX_CONTEXT_CHARS = 40  # an n-gram model only looks a few words back


def segment(text: str, lexicon: Lexicon, lm: MixtureLM) -> list[str]:
    """Unigram-Viterbi segmentation into dictionary words (other runs kept whole)."""
    tokens: list[str] = []
    for m in _RE_PIECE.finditer(text):
        run = m.group(1)
        if not run:
            tokens.append(m.group(0))
            continue
        n = len(run)
        best = [0.0] + [-math.inf] * n
        back = [0] * (n + 1)
        for i in range(n):
            if best[i] == -math.inf:
                continue
            for j in range(i + 1, min(n, i + MAX_WORD_LEN) + 1):
                w = run[i:j]
                if j - i > 1 and w not in lexicon:
                    continue
                s = best[i] + (lm.unigram(w) if w in lexicon else -30.0)
                if s > best[j]:
                    best[j], back[j] = s, i
        cut = []
        j = n
        while j > 0:
            cut.append(run[back[j]:j])
            j = back[j]
        tokens.extend(reversed(cut))
    return tokens


def context_state(text: str, lexicon: Lexicon, lm: MixtureLM) -> State:
    """LM state after reading the context (the current clause only)."""
    state = lm.begin()
    if not text:
        return state
    tail = _RE_SENTENCE_END.split(text[-MAX_CONTEXT_CHARS:])[-1]
    prev = None
    for tok in segment(tail, lexicon, lm):
        mapped = lm.map_token(tok, lexicon.__contains__)
        if mapped == PUNCT and prev in (PUNCT, None):
            continue
        _, state = lm.score(state, mapped)
        prev = mapped
    return state
