"""Assign a pronunciation to every character of a segmented sentence.

The corpus is plain text; the neural model needs (bopomofo syllable, character)
pairs. Pronunciations come from the dictionary:

  * a word with one reading uses it;
  * a word with several readings uses the one whose characters agree most with
    the single-character defaults, then the dictionary priority; with
    probability ALT_SHARE another spelling of the same reading is used instead
    (neutral tone written with its citation tone, or the reverse), because
    typists use both;
  * a one-character token uses single_char_readings.tsv when listed (a reading
    is drawn at random by share), otherwise a fixed default: the hand-set
    default if there is one, else the reading the character has most often
    inside unambiguous corpus words.

Variant readings (":var") are used only when a word has nothing else.
"""

from __future__ import annotations

import random
from collections import Counter, defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
PRIORITY = {"concised": 0, "idiom": 1, "revised": 2, "manual": 3, "web": 4}
ALT_SHARE = 0.3
PREFERRED_FORMS = str.maketrans({  # keep identical to training/lm/prepare.py
    "臺": "台", "裏": "裡", "爲": "為", "啓": "啟", "綫": "線", "衞": "衛", "峯": "峰",
    "羣": "群", "敍": "敘", "説": "說", "産": "產", "奬": "獎", "滙": "匯", "恒": "恆", "麽": "麼",
})


def _read_pairs(path: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines():
            parts = line.split("\t")
            if len(parts) == 2 and len(parts[0]) == 1 and not line.startswith("#"):
                out.setdefault(parts[0], parts[1])
    return out


class Readings:
    def __init__(self, dict_tsv: Path, counts_tsv: Path | None = None):
        entries: dict[str, list[tuple[tuple[str, ...], str]]] = defaultdict(list)
        with open(dict_tsv, encoding="utf-8") as fh:
            for line in fh:
                parts = line.rstrip("\n").split("\t")
                if len(parts) >= 2:
                    entries[parts[0]].append((tuple(parts[1].split()), parts[2] if len(parts) > 2 else ""))
        self.entries = dict(entries)

        # Fixed single-character defaults.
        default = _read_pairs(REPO / "eval" / "tools" / "char_defaults.tsv")
        for ch, r in _read_pairs(REPO / "training" / "dict" / "manual_readings.tsv").items():
            default.setdefault(ch, r)
        in_word: dict[str, Counter] = defaultdict(Counter)
        if counts_tsv and counts_tsv.exists():
            with open(counts_tsv, encoding="utf-8") as fh:
                for line in fh:
                    word, _, n = line.rstrip("\n").partition("\t")
                    if len(word) > 1 and word in self.entries:
                        plain = [r for r, tag in self.entries[word] if ":" not in tag]
                        if len(plain) == 1:
                            for ch, syl in zip(word, plain[0]):
                                in_word[ch][syl] += int(n)
        self.char_default: dict[str, str] = {}
        for word, rs in self.entries.items():
            if len(word) != 1:
                continue
            opts = [r[0] for r, tag in rs if ":var" not in tag] or [r[0] for r, _ in rs]
            hand = default.get(word)
            if hand in opts:
                self.char_default[word] = hand
            else:
                self.char_default[word] = max(opts, key=lambda s: (in_word[word][s], -opts.index(s)))

        # Characters whose stand-alone reading is drawn at random.
        self.char_choice: dict[str, tuple[list[str], list[float]]] = {}
        table: dict[str, list[tuple[str, float]]] = defaultdict(list)
        for line in (HERE / "single_char_readings.tsv").read_text(encoding="utf-8").splitlines():
            parts = line.split("\t")
            if len(parts) == 3 and not line.startswith("#"):
                table[parts[0]].append((parts[1], float(parts[2])))
        for ch, opts in table.items():
            self.char_choice[ch] = ([r for r, _ in opts], [p for _, p in opts])

        # Multi-character words: (main reading, alternative spellings).
        self._word: dict[str, tuple[tuple[str, ...], list[tuple[str, ...]]]] = {}

    def __contains__(self, word: str) -> bool:
        return word in self.entries

    def _word_options(self, word: str):
        hit = self._word.get(word)
        if hit is None:
            rs = self.entries[word]
            main = [(r, tag) for r, tag in rs if ":" not in tag]
            alts = [r for r, tag in rs if tag.endswith(":alt")]
            if not main:
                main = [(r, tag) for r, tag in rs if not tag.endswith(":alt")] or rs
            agree = lambda r: sum(self.char_default.get(c) == s for c, s in zip(word, r))
            best = min(main, key=lambda x: (-agree(x[0]), PRIORITY.get(x[1].split(":")[0], 9)))[0]
            # Other spellings of the chosen reading: the same syllables except that a
            # neutral tone is written with its citation tone, or the other way round.
            base = lambda syl: syl.strip("˙ˊˇˋ")
            same = lambda a: a != best and all(
                x == y or (base(x) == base(y) and (x.startswith("˙") or y.startswith("˙")))
                for x, y in zip(a, best))
            alts = [a for a in dict.fromkeys(alts + [r for r, _ in main]) if same(a)]
            hit = self._word[word] = (best, alts)
        return hit

    def read(self, word: str, rng: random.Random) -> tuple[str, ...] | None:
        """Syllables for a dictionary word (one per character), or None if unknown."""
        if word not in self.entries:
            return None
        if len(word) == 1:
            choice = self.char_choice.get(word)
            if choice is not None:
                return (rng.choices(choice[0], choice[1])[0],)
            return (self.char_default[word],)
        best, alts = self._word_options(word)
        if alts and rng.random() < ALT_SHARE:
            return rng.choice(alts)
        return best
