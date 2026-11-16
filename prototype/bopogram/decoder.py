"""Lattice decoding: bopomofo syllables -> most probable word sequence.

The lattice has one node per syllable boundary. An edge from i to j is a
dictionary word pronounced as syllables[i:j]. Hypotheses ending at the same
node with the same language-model state are merged (Viterbi); each node keeps
at most `beam` hypotheses.

Decoding is incremental: adding a syllable only builds the new last node
from the previous ones, so the cost per keystroke does not grow with the
length of the composition buffer. Editing in the middle of the buffer
rebuilds only the nodes after the edit.

The buffer may also hold literal characters (punctuation, spaces, letters
typed in between). A literal is stored as "=" + character (see `literal`);
it occupies one position, is never converted, and no word spans across it.
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
LITERAL = "="

# Tone sandhi: 一 and 不 change tone before certain tones, and people type what they
# say. The single character may be typed with its spoken tone only when the next
# syllable has a tone that causes the change: (character, typed syllable) -> tones.
# Until the next syllable arrives the choice carries SANDHI_PENALTY, so that
# ㄧˋ typed on its own is still 意 / 億 / 易 rather than 一.
SANDHI = {("一", "ㄧˊ"): {4}, ("一", "ㄧˋ"): {1, 2, 3}, ("不", "ㄅㄨˊ"): {4}}
SANDHI_ALSO = {("一", "ㄧˊ"): {"˙ㄍㄜ"}}  # 一個 with a neutral-tone 個
SANDHI_PENALTY = -4.0


_SANDHI_SYLLABLES = {syl for _, syl in SANDHI}


def tone_of(syllable: str) -> int:
    """1-4, or 5 for the neutral tone."""
    if syllable.startswith("˙"):
        return 5
    return {"ˊ": 2, "ˇ": 3, "ˋ": 4}.get(syllable[-1:], 1)


def literal(char: str) -> str:
    """Buffer item for a character that is passed through unchanged."""
    return LITERAL + char


def is_literal(item: str) -> bool:
    return item.startswith(LITERAL)


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
        # Optional per-position character scores from the neural model (see set_position_scores).
        self.position_scores: list[dict[str, float]] | None = None
        self.position_weight = 0.0
        self.position_floor = -12.0
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
        if cands is None and any(is_literal(item) for item in key):
            return [key[0][len(LITERAL):]] if len(key) == 1 else []
        if cands is None:
            cands = self.lexicon.candidates(key)
            # Words the language model has never seen (rare characters, bopomofo symbols)
            # cannot win on their own; leave them out unless nothing else fits. They stay
            # in the candidate list, and a word picked by hand is always used.
            floor = self.lm.unigram("<unk>") + 1e-6
            known = [w for w in cands if self.lm.unigram(w) > floor]
            cands = known or cands
            if len(cands) > self.max_candidates:
                rank = self.lm.unigram
                if len(key) == 1:
                    rank = lambda w: self.lm.unigram(w) + self.lexicon.reading_logp(w, key[0])
                cands = sorted(cands, key=rank, reverse=True)[: self.max_candidates]
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

    def set_position_scores(self, scores: list[dict[str, float]] | None, weight: float = 1.0,
                            floor: float = -12.0) -> None:
        """Per-position character log-probabilities to add to the lattice.

        scores[k] maps a character to its log-probability at syllable k. The sum over
        a word's characters, times `weight`, is added to every edge. A character the
        model did not score, or scored below `floor`, counts as `floor`: the model can
        make a dictionary word less likely but can never remove it.
        Takes effect for syllables pushed afterwards.
        """
        self.position_scores = scores
        self.position_weight = weight
        self.position_floor = floor

    def rescore(self, scores: list[dict[str, float]] | None, weight: float = 1.0,
                floor: float = -12.0) -> None:
        """Replace the position scores and decode the whole buffer again. Needed after
        every change of the buffer, because the neural model reads all of it."""
        self.set_position_scores(scores, weight, floor)
        self._rebuild(0, list(self.syllables))

    def _position_score(self, i: int, word: str) -> float:
        scores, floor = self.position_scores, self.position_floor
        total = 0.0
        for k, ch in enumerate(word, i):
            if k < len(scores):
                total += max(scores[k].get(ch, floor), floor)
        return total * self.position_weight

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
            # A 一 / 不 typed with its sandhi tone just before node i is only valid if the
            # syllable at i has the right tone; if so its provisional penalty is returned.
            refund: dict[int, float] = {}
            if i > 0 and self.syllables[i - 1] in _SANDHI_SYLLABLES:
                nxt, keep = self.syllables[i], []
                for h in hyps:
                    rule = (h.word, self.syllables[i - 1]) if h.start == i - 1 else None
                    if rule in SANDHI:
                        ok = not is_literal(nxt) and (tone_of(nxt) in SANDHI[rule]
                                                      or nxt in SANDHI_ALSO.get(rule, ()))
                        if not ok:
                            continue
                        refund[id(h)] = -SANDHI_PENALTY
                    keep.append(h)
                hyps = keep
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
                bonus = self._position_score(i, word) if self.position_scores is not None else 0.0
                # A literal is scored as the token class the model was trained with
                # (punctuation, Latin, digits); a space does not touch the model.
                token = word
                if j - i == 1 and is_literal(key[0]):
                    token = None if word.isspace() else self.lm.map_token(word, self.lexicon.__contains__)
                if j - i == 1:
                    if (word, key[0]) in SANDHI:
                        bonus += SANDHI_PENALTY  # sandhi tones have their own rule
                    elif token is word:
                        # P(word, reading | history) = P(word | history) * P(reading | word):
                        # a character is less likely under a reading it seldom has.
                        bonus += self.lexicon.reading_logp(word, key[0])
                for h in hyps:
                    logp, state = score_fn(h.state, token) if token is not None else (0.0, h.state)
                    total = h.score + logp + self.word_penalty + bonus
                    if refund:
                        total += refund.get(id(h), 0.0)
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
            self.delete(len(self.syllables) - 1)

    def _rebuild(self, keep: int, items: list[str]) -> None:
        """Keep the first `keep` items' nodes and push the rest of `items` again."""
        self.syllables = items[:keep]
        del self.nodes[keep + 1:]
        for item in items[keep:]:
            self.push(item)

    def insert(self, pos: int, item: str) -> None:
        """Insert a syllable or literal before position `pos`.
        Pins after it move along; a pin it would split is dropped."""
        if pos >= len(self.syllables) and not self.pinned:
            self.push(item)
            return
        keep, pinned = pos, {}
        for (a, b), w in self.pinned.items():
            if b <= pos:
                pinned[(a, b)] = w
            elif a >= pos:
                pinned[(a + 1, b + 1)] = w
            else:
                keep = min(keep, a)
        items = self.syllables[:pos] + [item] + self.syllables[pos:]
        self.pinned = pinned
        self._rebuild(keep, items)

    def delete(self, pos: int) -> None:
        """Remove the item at position `pos`; pins covering it are dropped."""
        if not 0 <= pos < len(self.syllables):
            return
        keep, pinned = pos, {}
        for (a, b), w in self.pinned.items():
            if b <= pos:
                pinned[(a, b)] = w
            elif a > pos:
                pinned[(a - 1, b - 1)] = w
            else:
                keep = min(keep, a)
        items = self.syllables[:pos] + self.syllables[pos + 1:]
        self.pinned = pinned
        self._rebuild(keep, items)

    def drop_front(self, count: int, start_state: State) -> None:
        """Remove the first `count` items (they were committed) and decode the rest
        again after `start_state`."""
        items = self.syllables[count:]
        self.pinned = {(a - count, b - count): w for (a, b), w in self.pinned.items() if a >= count}
        self.syllables = []
        self.nodes = [[Hyp(0.0, start_state, None, 0, None)]]
        for item in items:
            self.push(item)

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
