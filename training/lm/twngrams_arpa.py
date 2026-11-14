"""Turn the twngrams web n-gram counts into a small back-off language model.

Usage:
    uv run python lm/twngrams_arpa.py TWNGRAMS_DIR DICT_TSV OUT_ARPA

twngrams counts the number of websites using an n-gram, not occurrences, and
only lists n-grams above a threshold. That is not what n-gram estimators
expect, so the model here is deliberately simple:

    P(w)        = c(w) / sum c
    P(w | h)    = ALPHA_n * c(h w) / sum_w' c(h w')        for listed n-grams
                  backoff(h) * P(w | shorter h)            otherwise

ALPHA_n is the share of probability given to listed continuations; the rest
goes to the back-off, because most real continuations are below the listing
threshold. The model is only used as one component of a mixture whose weight
is fitted on the development set, so its absolute calibration matters little.

N-grams containing a token that is not a dictionary word are dropped.
"""

from __future__ import annotations

import math
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from prepare import PREFERRED_FORMS, load_vocab  # noqa: E402

ALPHA = {2: 0.6, 3: 0.5}
UNK_PROB = 1e-7


def read_counts(path: Path, n: int, vocab: set[str]) -> dict[tuple[str, ...], int]:
    out: dict[tuple[str, ...], int] = {}
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            count, _, rest = line.rstrip("\n").partition("\t")
            toks = tuple(t.translate(PREFERRED_FORMS) for t in rest.split(" "))
            if len(toks) == n and count.isdigit() and all(t in vocab for t in toks):
                out[toks] = out.get(toks, 0) + int(count)
    return out


def main() -> None:
    src, dict_tsv, out_path = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
    vocab = load_vocab(dict_tsv)
    c1 = read_counts(src / "token_1gram.tsv", 1, vocab)
    c2 = read_counts(src / "token_2gram.tsv", 2, vocab)
    c3 = read_counts(src / "token_3gram.tsv", 3, vocab)
    # Higher orders may mention words missing from the 1-gram list.
    for grams in (c2, c3):
        for toks in grams:
            for t in toks:
                c1.setdefault((t,), 1)

    total = sum(c1.values())
    p1 = {w[0]: (1 - 3 * UNK_PROB) * c / total for w, c in c1.items()}

    def conditional(counts, alpha):
        z: dict[tuple[str, ...], int] = defaultdict(int)
        for toks, c in counts.items():
            z[toks[:-1]] += c
        return {toks: alpha * c / z[toks[:-1]] for toks, c in counts.items()}

    p2 = conditional(c2, ALPHA[2])
    # Keep only trigrams whose context is a listed bigram (ARPA requires it).
    c3 = {t: c for t, c in c3.items() if t[:2] in p2 and t[1:] in p2}
    p3 = conditional(c3, ALPHA[3])

    def backoffs(high, lower_prob):
        seen_high: dict[tuple[str, ...], float] = defaultdict(float)
        seen_low: dict[tuple[str, ...], float] = defaultdict(float)
        for toks, p in high.items():
            seen_high[toks[:-1]] += p
            seen_low[toks[:-1]] += lower_prob(toks[1:])
        return {h: (1 - seen_high[h]) / max(1e-9, 1 - seen_low[h]) for h in seen_high}

    bow1 = backoffs(p2, lambda t: p1[t[0]])
    bow2 = backoffs(p3, lambda t: p2[t])

    lg = math.log10
    with open(out_path, "w", encoding="utf-8") as fh:
        fh.write(f"\\data\\\nngram 1={len(p1) + 3}\nngram 2={len(p2)}\nngram 3={len(p3)}\n\n\\1-grams:\n")
        fh.write(f"{lg(UNK_PROB):.5f}\t<unk>\n-99\t<s>\t0\n{lg(UNK_PROB):.5f}\t</s>\n")
        for w, p in p1.items():
            fh.write(f"{lg(p):.5f}\t{w}\t{lg(bow1.get((w,), 1.0)):.5f}\n")
        fh.write("\n\\2-grams:\n")
        for toks, p in p2.items():
            fh.write(f"{lg(p):.5f}\t{' '.join(toks)}\t{lg(bow2.get(toks, 1.0)):.5f}\n")
        fh.write("\n\\3-grams:\n")
        for toks, p in p3.items():
            fh.write(f"{lg(p):.5f}\t{' '.join(toks)}\n")
        fh.write("\n\\end\\\n")
    print(f"1-grams {len(p1)}, 2-grams {len(p2)}, 3-grams {len(p3)}")


if __name__ == "__main__":
    main()
