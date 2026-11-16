"""Estimate P(reading | character) for single characters, for the engine.

Usage:
    uv run python neural/reading_counts.py DATA_DIR DICT_TSV COUNTS_TSV OUT_TSV

DATA_DIR is the output of make_data.py. Output: character <TAB> syllable <TAB>
probability. The engine uses it so that a character is not offered first under
a reading it almost never has (混 under ㄎㄨㄣ).

The estimate is the average of two views, because neither is right alone:
  * every occurrence in the corpus, inside words too (counted from the arrays);
    this knows that 混 is hardly ever ㄎㄨㄣ, but underrates readings a character
    mostly has when it stands alone (地 as ˙ㄉㄜ, 教 as ㄐㄧㄠ);
  * the character standing alone, as readings.py assigns it (the hand-made
    shares of single_char_readings.tsv, or the fixed default).
Shards are memory-mapped and counted one at a time.
"""

from __future__ import annotations

import json
import os
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from readings import Readings  # noqa: E402


def main() -> None:
    data_dir, dict_tsv, counts_tsv, out = (Path(a) for a in sys.argv[1:5])
    vocab = json.loads((data_dir / "vocab.json").read_text(encoding="utf-8"))
    n_syl = len(vocab["syllables"])
    total = np.zeros(len(vocab["chars"]) * n_syl, dtype=np.int64)
    for chars_path in sorted(data_dir.glob("*/part-*.chars.npy")):
        chars = np.load(chars_path, mmap_mode="r")
        syls = np.load(str(chars_path).replace(".chars.npy", ".syls.npy"), mmap_mode="r")
        for a in range(0, len(chars), 20_000_000):
            c = np.asarray(chars[a:a + 20_000_000], dtype=np.int64)
            s = np.asarray(syls[a:a + 20_000_000], dtype=np.int64)
            keep = s > 0
            total += np.bincount(c[keep] * n_syl + s[keep], minlength=len(total))
    seen: dict[str, dict[str, int]] = defaultdict(dict)
    for idx in np.flatnonzero(total):
        seen[vocab["chars"][idx // n_syl]][vocab["syllables"][idx % n_syl]] = int(total[idx])

    readings = Readings(dict_tsv, counts_tsv)
    rows = []
    for ch, in_corpus in seen.items():
        n = sum(in_corpus.values())
        if ch in readings.char_choice:
            alone = dict(zip(*readings.char_choice[ch]))
        else:
            alone = {readings.char_default.get(ch, max(in_corpus, key=in_corpus.get)): 1.0}
        for syl in set(in_corpus) | set(alone):
            rows.append((ch, syl, 0.5 * in_corpus.get(syl, 0) / n + 0.5 * alone.get(syl, 0.0), n))
    rows.sort(key=lambda r: (-r[3], r[0], -r[2]))
    tmp = out.with_suffix(".tmp")
    with open(tmp, "w", encoding="utf-8") as fh:
        for ch, syl, p, _ in rows:
            fh.write(f"{ch}\t{syl}\t{p:.6f}\n")
    os.replace(tmp, out)
    print(f"{len(rows)} (character, reading) pairs for {len(seen)} characters")


if __name__ == "__main__":
    main()
