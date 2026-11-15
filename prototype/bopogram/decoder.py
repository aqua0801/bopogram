"""Lattice decoding: bopomofo syllables -> most probable word sequence.

The lattice has one node per syllable boundary. An edge from i to j is a
dictionary word pronounced as syllables[i:j]. Hypotheses ending at the same
node with the same language-model state are merged (Viterbi); each node keeps
at most `beam` hypotheses.

Decoding is incremental: adding a syllable only builds the new last node
from the previous ones, so the cost per keystroke does not grow with the
length of the composition buffer.
"""

from __future__ import annotations

from dataclasses import dataclass

from .lexicon import MAX_WORD_LEN, Lexicon
from .lm import UNKNOWN_CHAR, MixtureLM, State

# The three structural particles share one pronunciation. Everyday writing uses
# 的 for all of them; formal writing distinguishes 的 / 地 / 得.
PARTICLE_SYLLABLE = ("˙ㄉㄜ",)
PARTICLE_CASUAL = ["的"]
STYLES = ("casual", "formal")


@dataclass(slots=True)
class Hyp:
    score: float
    state: State
    word: str | None      # last word (None for the start hypothesis)
    start: int            # node where `word` starts
    prev: "Hyp | None"


class Decoder:
    def __init__(self, lexicon: Lexicon, lm: MixtureLM, beam: int = 16, max_candidates: int = 40,
                 extra_score=None, word_penalty: float = 0.0, eos_weight: float = 0.0,
                 style: str = "formal"):
        self.lexicon = lexicon
        self.lm = lm
        self.beam = beam
        self.max_candidates = max_candidates
        self.extra_score = extra_score  # optional callable(prev_word, word) -> added log score
        self.word_penalty = word_penalty  # added per word; negative values favour longer words
        self.eos_weight = eos_weight      # weight of ln P(end of sentence) when ranking results
        # "casual": a lone neutral-tone ㄉㄜ is always 的 unless the user picks otherwise.
        # "formal": the language model chooses between 的 / 地 / 得.
        self.style = style
        self._cand_cache: dict[tuple[str, ...], list[str]] = {}
        # Spans the user has fixed by picking a candidate: (start, end) -> word.
        self.pinned: dict[tuple[int, int], str] = {}
        self.reset()

    # -- composition buffer ---------------------------------------------
    def reset(self, start_state: State | None = None) -> None:
        self.syllables: list[str] = []
        self.pinned = {}
        first = Hyp(0.0, start_state if start_state is not None else self.lm.begin(), None, 0, None)
        self.nodes: list[list[Hyp]] = [[first]]

    def _candidates(self, key: tuple[str, ...]) -> list[str]:
        cands = self._cand_cache.get(key)
        if cands is None:
            cands = self.lexicon.candidates(key)
            if len(cands) > self.max_candidates:
                cands = sorted(cands, key=self.lm.unigram, reverse=True)[: self.max_candidates]
            self._cand_cache[key] = cands
        return cands

    def _allowed(self, i: int, j: int, word: str) -> bool:
        """An edge may not cut through a pinned span, and must equal it if it covers it."""
        for (a, b), fixed in self.pinned.items():
            if j <= a or i >= b:
                continue
            if (i, j) != (a, b) or word != fixed:
                return False
        return True

    def set_style(self, style: str) -> None:
        """Switch between "casual" and "formal" and decode the current buffer again."""
        if style not in STYLES:
            raise ValueError(f"unknown style {style!r}")
        if style != self.style:
            self.style = style
            self.repin(self.pinned)

    def repin(self, pinned: dict[tuple[int, int], str], start_state: State | None = None) -> None:
        """Replace the pinned spans and decode the current buffer again."""
        syllables = list(self.syllables)
        if start_state is None:
            start_state = self.nodes[0][0].state
        self.pinned = dict(pinned)
        self.syllables = []
        self.nodes = [[Hyp(0.0, start_state, None, 0, None)]]
        for s in syllables:
            self.push(s)

    def push(self, syllable: str) -> None:
        """Append one syllable and build the lattice node after it."""
        self.syllables.append(syllable)
        j = len(self.syllables)
        merged: dict[State, Hyp] = {}
        score_fn, extra = self.lm.score, self.extra_score
        for i in range(max(0, j - MAX_WORD_LEN), j):
            hyps = self.nodes[i]
            if not hyps:
                continue
            key = tuple(self.syllables[i:j])
            words = PARTICLE_CASUAL if self.style == "casual" and key == PARTICLE_SYLLABLE \
                else self._candidates(key)
            if self.pinned:
                fixed = self.pinned.get((i, j))
                if fixed is not None:
                    words = [fixed]
                else:
                    words = [w for w in words if self._allowed(i, j, w)]
            for word in words:
                for h in hyps:
                    logp, state = score_fn(h.state, word)
                    total = h.score + logp + self.word_penalty
                    if extra is not None:
                        total += extra(h.word, word)
                    old = merged.get(state)
                    if old is None or total > old.score:
                        merged[state] = Hyp(total, state, word, i, h)
        if not merged:
            # No dictionary word ends here (e.g. a syllable the dictionary lacks):
            # keep decoding by passing the raw syllable through as an unknown token.
            for h in self.nodes[j - 1]:
                logp, state = score_fn(h.state, UNKNOWN_CHAR)
                merged.setdefault(state, Hyp(h.score + logp, state, syllable, j - 1, h))
        best = sorted(merged.values(), key=lambda h: h.score, reverse=True)[: self.beam]
        self.nodes.append(best)

    def pop(self) -> None:
        """Remove the last syllable (backspace); pins reaching into it are dropped."""
        if self.syllables:
            self.syllables.pop()
            self.nodes.pop()
            n = len(self.syllables)
            self.pinned = {span: w for span, w in self.pinned.items() if span[1] <= n}

    # -- results ----------------------------------------------------------
    def nbest(self, n: int = 1) -> list[tuple[list[str], float]]:
        """Up to n best segmentations of the whole buffer, with log scores."""
        out: list[tuple[list[str], float]] = []
        seen: set[str] = set()
        final = self.nodes[-1]
        if self.eos_weight and len(final) > 1:
            w, score_fn = self.eos_weight, self.lm.score
            final = sorted(final, key=lambda h: h.score + w * score_fn(h.state, "</s>")[0], reverse=True)
        for h in final:
            words: list[str] = []
            cur: Hyp | None = h
            while cur is not None and cur.word is not None:
                words.append(cur.word)
                cur = cur.prev
            words.reverse()
            text = "".join(words)
            if text in seen:
                continue
            seen.add(text)
            out.append((words, h.score))
            if len(out) == n:
                break
        return out

    def best_text(self) -> str:
        res = self.nbest(1)
        return "".join(res[0][0]) if res else ""

    def decode(self, syllables: list[str], start_state: State | None = None, n: int = 1):
        """Convenience: decode a whole syllable sequence at once."""
        self.reset(start_state)
        for s in syllables:
            self.push(s)
        return self.nbest(n)
