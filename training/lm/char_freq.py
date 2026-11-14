"""Character frequencies from segmentation word counts (for the char-unigram baseline).

Usage:
    uv run python lm/char_freq.py COUNTS_TSV OUT_TSV
"""

import sys
from collections import Counter

counts: Counter = Counter()
with open(sys.argv[1], encoding="utf-8") as fh:
    for line in fh:
        word, _, n = line.rstrip("\n").partition("\t")
        if n.isdigit():
            for ch in word:
                counts[ch] += int(n)
with open(sys.argv[2], "w", encoding="utf-8") as out:
    for ch, n in counts.most_common():
        out.write(f"{ch}\t{n}\n")
