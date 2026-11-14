"""Build the Bopogram pronunciation dictionary (word -> bopomofo) locally.

Usage:
    uv run python dict/build_dict.py MOE_DIR OUT_DIR [--twngrams DIR] [--manual FILE]

Inputs (downloaded by the user / pipeline, never committed):
  MOE_DIR    unpacked Ministry of Education dictionary spreadsheets
             (dict_revised_*.xlsx, dict_concised_*.xlsx, dict_idioms_*.xlsx).
             Licence CC BY-ND 3.0 TW: the output is for local use only and
             must not be redistributed.
  --twngrams directory with token_1gram.tsv (CC0), used to add current words
             that the dictionaries lack.
  --manual   TSV of hand-assigned readings for added words (word, bopomofo).

Outputs in OUT_DIR:
  dict.tsv            word <TAB> syllables separated by spaces <TAB> source tag
  rejected.tsv        entries that could not be aligned (for inspection)
  need_reading.tsv    added words whose reading is ambiguous (to label by hand)
  stats.json

Format rules (agreed): one syllable per character; first tone unmarked;
neutral tone written with a leading "˙"; a word with several readings
appears on several lines. Source tags: concised, idiom, revised, web, manual,
with ":var" appended for variant readings (sandhi, alternative, colloquial,
literary) and ":alt" for the citation-tone spelling of a neutral-tone word.
"""

from __future__ import annotations

import argparse
import itertools
import json
import re
from collections import Counter, defaultdict
from pathlib import Path

import openpyxl

SYLLABLE = re.compile(r"^˙?[ㄅ-ㄩ]{1,3}[ˊˇˋ]?$")
CJK_WORD = re.compile(r"^[一-鿿㐀-䶿]+$")
PRIORITY = {"concised": 0, "idiom": 1, "revised": 2, "manual": 3, "web": 4}
MAX_WORD_LEN = 8
ERHUA_READING = "ㄦˊ"
DOMINANCE_SHARE = 0.9     # a reading used this often in a position is taken as certain
DOMINANCE_MIN_COUNT = 3
EVERYDAY_FORMS = str.maketrans({"臺": "台"})


def split_syllables(field: str) -> list[str]:
    """MOE bopomofo field -> syllables. Detaches a suffixed erhua 'ㄦ'."""
    out = []
    for tok in re.split(r"[\s　]+", field.strip()):
        if not tok:
            continue
        m = re.match(r"^(˙?[ㄅ-ㄩ]{1,3}[ˊˇˋ]?)ㄦ$", tok)
        if m and m.group(1) not in ("", "˙"):
            out += [m.group(1), ERHUA_READING]
        else:
            out.append(tok)
    return out


def readings_from(field: str | None) -> list[list[str]]:
    """A field may hold several readings separated by markers such as （變）."""
    if not field:
        return []
    parts = re.split(r"[（(][^）)]*[）)]", str(field))
    return [s for s in (split_syllables(p) for p in parts) if s]


def clean_word(raw) -> str:
    word = str(raw or "").strip()
    return re.sub(r"[（(].*?[）)]|\d+$", "", word)  # drop sense markers like 行(1)


def iter_moe(path: Path, word_col: str, bpmf_col: str, var_col: str | None):
    wb = openpyxl.load_workbook(path, read_only=True)
    ws = wb.worksheets[0]
    rows = ws.iter_rows(values_only=True)
    header = [str(c).strip() if c is not None else "" for c in next(rows)]
    wi, bi = header.index(word_col), header.index(bpmf_col)
    vi = header.index(var_col) if var_col else None
    for row in rows:
        if row[wi] is None:
            continue
        main = readings_from(row[bi])
        variants = readings_from(row[vi]) if vi is not None else []
        # In the idiom table variants follow the main reading in the same field.
        yield clean_word(row[wi]), main[:1], main[1:] + variants
    wb.close()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("moe_dir", type=Path)
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--twngrams", type=Path)
    ap.add_argument("--manual", type=Path)
    args = ap.parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)

    def find(pattern: str) -> Path:
        hits = [p for p in sorted(args.moe_dir.rglob(pattern)) if "欄位說明" not in p.name]
        if not hits:
            raise SystemExit(f"missing {pattern} under {args.moe_dir}")
        return hits[-1]

    sources = [
        ("concised", find("dict_concised_*.xlsx"), "字詞名", "注音一式", "變體注音"),
        ("idiom", find("dict_idioms_*.xlsx"), "成語", "注音", None),
        ("revised", find("dict_revised_*.xlsx"), "字詞名", "注音一式", "變體注音"),
    ]

    entries: dict[tuple[str, tuple[str, ...]], str] = {}
    rejected: list[tuple[str, str, str, str]] = []
    stats: Counter = Counter()

    def add(word: str, syls: list[str], tag: str) -> bool:
        if not CJK_WORD.match(word) or len(word) > MAX_WORD_LEN:
            stats["skipped_non_cjk_or_long"] += 1
            return False
        if len(syls) != len(word) or not all(SYLLABLE.match(s) for s in syls):
            rejected.append((word, " ".join(syls), tag, "misaligned"))
            return False
        key = (word, tuple(syls))
        old = entries.get(key)
        if old is None or PRIORITY[tag.split(":")[0]] < PRIORITY[old.split(":")[0]] \
                or (old.endswith(":var") and not tag.endswith(":var")
                    and PRIORITY[tag.split(":")[0]] <= PRIORITY[old.split(":")[0]]):
            entries[key] = tag
        return True

    for tag, path, wcol, bcol, vcol in sources:
        for word, main, variants in iter_moe(path, wcol, bcol, vcol):
            stats[f"{tag}_rows"] += 1
            for syls in main:
                add(word, syls, tag)
            for syls in variants:
                add(word, syls, tag + ":var")
        print(f"{tag}: {stats[f'{tag}_rows']} rows, {len(entries)} entries so far", flush=True)

    # Per-character readings, used to give readings to added words.
    char_readings: dict[str, dict[str, str]] = defaultdict(dict)
    for (word, syls), tag in entries.items():
        if len(word) == 1:
            char_readings[word][syls[0]] = tag
    # "Common" readings: non-variant readings listed by the concise dictionary.
    common = {c: [s for s, t in r.items() if t == "concised"] for c, r in char_readings.items()}
    # How each character is read inside everyday words, by position (word-final
    # or not): e.g. 了 is ˙ㄌㄜ at the end of most words but ㄌㄧㄠˇ elsewhere.
    usage: dict[tuple[str, bool], Counter] = defaultdict(Counter)
    for (word, syls), tag in entries.items():
        if len(word) >= 2 and tag in ("concised", "idiom"):
            for i, (ch, syl) in enumerate(zip(word, syls)):
                usage[(ch, i == len(word) - 1)][syl] += 1
    overrides: dict[str, str] = {}

    # Neutral-tone syllables: many people type the character's own tone instead
    # (東西 as ㄒㄧ, 便宜 as ㄧˊ, 這個 as ㄍㄜˋ). Accept both by adding an ":alt" reading.
    def toneless(s: str) -> str:
        return s.lstrip("˙").rstrip("ˊˇˋ")

    alt_added = 0
    for (word, syls), tag in list(entries.items()):
        if len(word) < 2 or tag.endswith(":var") or not any(s.startswith("˙") for s in syls):
            continue
        options = []
        for ch, syl in zip(word, syls):
            alts = [syl]
            if syl.startswith("˙"):
                same_base = [r for r in char_readings.get(ch, {})
                             if not r.startswith("˙") and toneless(r) == toneless(syl)]
                alts = ([r for r in same_base if r in common.get(ch, [])] or same_base) or [syl]
            options.append(alts)
        combos = list(itertools.islice(itertools.product(*options), 6))
        for new in combos:
            key = (word, tuple(new))
            if tuple(new) != syls and key not in entries:
                entries[key] = tag.split(":")[0] + ":alt"
                alt_added += 1
    stats["neutral_tone_alternatives"] = alt_added

    def guess(ch: str, final: bool) -> list[str]:
        """Candidate readings of `ch` in an added word; one item means confident."""
        if ch in overrides:
            return [overrides[ch]]
        seen = usage.get((ch, final)) or Counter()
        total = sum(seen.values())
        if total >= DOMINANCE_MIN_COUNT:
            top, n = seen.most_common(1)[0]
            if n / total >= DOMINANCE_SHARE:
                return [top]
        opts = common.get(ch) or list(char_readings.get(ch, {}))
        if len(opts) == 1:
            return opts
        ranked = [s for s, _ in seen.most_common()]
        return ranked + [s for s in opts if s not in ranked]

    if args.manual and args.manual.exists():
        for line in args.manual.read_text(encoding="utf-8").splitlines():
            if line and not line.startswith("#"):
                word, bpmf = line.split("\t")[:2]
                add(word, bpmf.split(), "manual")
                stats["manual"] += 1
                if len(word) == 1:  # a single-character line also fixes that character's default
                    overrides[word] = bpmf.strip()

    need: list[tuple[str, int, str]] = []
    if args.twngrams:
        known = {w for w, _ in entries}
        with open(args.twngrams / "token_1gram.tsv", encoding="utf-8") as fh:
            for line in fh:
                count, _, word = line.rstrip("\n").partition("\t")
                if not CJK_WORD.match(word) or not 2 <= len(word) <= 4 or word in known:
                    continue
                options = [guess(c, i == len(word) - 1) for i, c in enumerate(word)]
                if all(len(o) == 1 for o in options):
                    add(word, [o[0] for o in options], "web")
                    stats["web_auto"] += 1
                else:
                    need.append((word, int(count), " ".join("/".join(o) or "?" for o in options)))
                    stats["web_need_reading"] += 1

    # Everyday spellings: the MOE tables write 臺, most people write 台. Give every
    # 臺-word a 台 twin with the same reading (training text is normalised the same way,
    # see PREFERRED_FORMS in training/lm/prepare.py).
    twins = 0
    for (word, syls), tag in list(entries.items()):
        twin = word.translate(EVERYDAY_FORMS)
        if twin != word and (twin, syls) not in entries:
            entries[(twin, syls)] = tag
            twins += 1
    stats["everyday_spelling_twins"] = twins

    ordered = sorted(entries.items(), key=lambda kv: (kv[0][0], kv[0][1]))
    with open(args.out_dir / "dict.tsv", "w", encoding="utf-8") as fh:
        for (word, syls), tag in ordered:
            fh.write(f"{word}\t{' '.join(syls)}\t{tag}\n")
    with open(args.out_dir / "rejected.tsv", "w", encoding="utf-8") as fh:
        for row in rejected:
            fh.write("\t".join(row) + "\n")
    with open(args.out_dir / "need_reading.tsv", "w", encoding="utf-8") as fh:
        for word, count, opts in sorted(need, key=lambda x: -x[1]):
            fh.write(f"{word}\t{count}\t{opts}\n")

    words = {w for w, _ in entries}
    stats.update({
        "entries": len(entries),
        "distinct_words": len(words),
        "single_chars": sum(1 for w in words if len(w) == 1),
        "distinct_syllables": len({s for _, syls in entries for s in syls}),
        "words_with_multiple_readings": sum(1 for n in Counter(w for w, _ in entries).values() if n > 1),
        "rejected": len(rejected),
    })
    stats.update({f"tag_{t}": n for t, n in Counter(entries.values()).items()})
    stats.update({f"len_{n}": c for n, c in Counter(min(len(w), 5) for w in words).items()})
    (args.out_dir / "stats.json").write_text(json.dumps(dict(stats), ensure_ascii=False, indent=1))
    print(json.dumps(dict(stats), ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
