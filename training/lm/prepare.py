"""Stream segmented text as language-model training lines (to stdout).

Usage:
    uv run python lm/prepare.py DICT_TSV SEGMENTED_SOURCE_DIR [...] | lmplz ...

Token mapping (must match bopogram.lm in the prototype engine):
    dictionary word                 -> itself (after PREFERRED_FORMS)
    digits / Latin-letter run       -> <n> / <e>
    Chinese character not in dict   -> <x>
    anything else (punctuation)     -> <p>   (runs collapsed, dropped at line ends)
Lines are read and written one at a time.
"""

from __future__ import annotations

import gzip
import re
import sys
from pathlib import Path

NUM, LATIN, UNKNOWN_CHAR, PUNCT = "<n>", "<e>", "<x>", "<p>"
# Variant characters in the corpus are rewritten to the form the input method
# should produce (臺 -> 台 is a deliberate choice of the everyday spelling; the
# rest map rare variants to the standard Taiwan form).
PREFERRED_FORMS = str.maketrans({
    "臺": "台", "裏": "裡", "爲": "為", "啓": "啟", "綫": "線", "衞": "衛", "峯": "峰",
    "羣": "群", "敍": "敘", "説": "說", "産": "產", "奬": "獎", "滙": "匯", "恒": "恆", "麽": "麼",
})
_RE_NUM = re.compile(r"^[0-9][0-9.%+\-]*$")
_RE_LATIN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9.%+\-]*$")
_RE_CJK = re.compile(r"^[一-鿿㐀-䶿]+$")


def load_vocab(dict_tsv: Path) -> set[str]:
    with open(dict_tsv, encoding="utf-8") as fh:
        return {line.split("\t", 1)[0] for line in fh}


def map_token(tok: str, vocab: set[str]) -> str:
    tok = tok.translate(PREFERRED_FORMS)
    if tok in vocab:
        return tok
    if _RE_NUM.match(tok):
        return NUM
    if _RE_LATIN.match(tok):
        return LATIN
    if _RE_CJK.match(tok):
        return UNKNOWN_CHAR
    return PUNCT


def map_line(tokens: list[str], vocab: set[str]) -> list[str]:
    out: list[str] = []
    for tok in tokens:
        m = map_token(tok, vocab)
        if m == PUNCT and (not out or out[-1] == PUNCT):
            continue
        out.append(m)
    if out and out[-1] == PUNCT:
        out.pop()
    return out


def main() -> None:
    vocab = load_vocab(Path(sys.argv[1]))
    write = sys.stdout.write
    for src in sys.argv[2:]:
        for shard in sorted(Path(src).glob("part-*.txt.gz")):
            with gzip.open(shard, "rt", encoding="utf-8") as fh:
                for line in fh:
                    out = map_line(line.split(), vocab)
                    if out:
                        write(" ".join(out) + "\n")


if __name__ == "__main__":
    main()
