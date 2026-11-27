"""Build the Bopogram pronunciation dictionary (word -> bopomofo).

Usage:
    uv run python dict/build_dict.py extra OUT_TSV [--twngrams DIR] [--manual FILE] [--cns DIR]
    uv run python dict/build_dict.py build MOE_DIR EXTRA_TSV OUT_DIR

Two steps, because the MOE dictionaries (CC BY-ND 3.0 TW) may be redistributed
only unchanged, with their usage notes, and the built dictionary may not
(prototype/docs/release-spec.md):

  extra  reads only the other sources and writes one file that may be distributed:
         twngrams candidate words (CC0), this project's manual readings and typing
         variants, CNS 11643 readings of Traditional characters (OpenCC drops
         simplified forms here, so `build` needs no OpenCC). See make_extra().
  build  reads the MOE spreadsheets, unpacked or inside the downloaded .zip files,
         and EXTRA_TSV, and writes the dictionary. Everything that depends on or
         adjusts MOE content happens here, on the user's machine: neutral-tone
         alternatives, readings guessed for added words, 臺/台 twins, typing
         variants, which CNS characters are missing, 一/不 sandhi. This is the
         step the Rust engine ports as `bopogram build-dict`; its output must be
         byte-identical.

Outputs of build in OUT_DIR:
  dict.tsv            word <TAB> syllables separated by spaces <TAB> source tag
  rejected.tsv        entries that could not be aligned (for inspection)
  need_reading.tsv    added words whose reading is ambiguous (to label by hand)
  stats.json

Format rules (agreed): one syllable per character; first tone unmarked;
neutral tone written with a leading "˙"; a word with several readings
appears on several lines. Source tags: concised, idiom, revised, web, manual,
"cns" (characters added from the CNS 11643 database) or "symbol" (a bopomofo
symbol standing for itself),
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


def open_xlsx(source):
    """A path to an .xlsx, or (zip path, member name): the archive as downloaded."""
    if isinstance(source, tuple):
        import io
        import zipfile

        with zipfile.ZipFile(source[0]) as z:
            return openpyxl.load_workbook(io.BytesIO(z.read(source[1])), read_only=True)
    return openpyxl.load_workbook(source, read_only=True)


def find_moe(moe_dir: Path, pattern: str):
    """The newest MOE table matching `pattern`, unpacked or inside its .zip."""
    import fnmatch
    import zipfile

    hits = [(p.name, p) for p in moe_dir.rglob(pattern) if "欄位說明" not in p.name]
    for z in moe_dir.rglob("*.zip"):
        with zipfile.ZipFile(z) as zf:
            hits += [(Path(n).name, (z, n)) for n in zf.namelist()
                     if fnmatch.fnmatch(Path(n).name, pattern) and "欄位說明" not in n]
    if not hits:
        raise SystemExit(f"missing {pattern} under {moe_dir}")
    return sorted(hits, key=lambda h: h[0])[-1][1]


def iter_moe(source, word_col: str, bpmf_col: str, var_col: str | None):
    wb = open_xlsx(source)
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


def make_extra(twngrams: Path | None, manual: Path | None, cns: Path | None, out: Path) -> None:
    """Everything the dictionary takes from sources other than the MOE tables, in one
    file that may be distributed (no MOE content): lines of
        twngrams <TAB> word <TAB> count     candidate words (CC0), 2-4 Chinese characters
        manual   <TAB> word <TAB> syllables  this project's hand-assigned readings
        variant  <TAB> char <TAB> reading <TAB> reading people also type
        cns      <TAB> char <TAB> syllable   CNS 11643 readings (Traditional characters only)
    Which of them are used depends on the MOE tables, so that is decided by `build`."""
    lines: list[str] = []
    if twngrams:
        with open(twngrams / "token_1gram.tsv", encoding="utf-8") as fh:
            for line in fh:
                count, _, word = line.rstrip("\n").partition("\t")
                if CJK_WORD.match(word) and 2 <= len(word) <= 4:
                    lines.append(f"twngrams\t{word}\t{count}")
    if manual and manual.exists():
        for line in manual.read_text(encoding="utf-8").splitlines():
            if line and not line.startswith("#"):
                word, bpmf = line.split("\t")[:2]
                lines.append(f"manual\t{word}\t{bpmf.strip()}")
    variants_file = Path(__file__).resolve().parent / "typing_variants.tsv"
    if variants_file.exists():
        for line in variants_file.read_text(encoding="utf-8").splitlines():
            parts = line.split("\t")
            if len(parts) == 3 and not line.startswith("#"):
                lines.append("variant\t" + "\t".join(parts))
    if cns:
        import opencc

        to_trad = opencc.OpenCC("s2t")
        code_to_char: dict[str, str] = {}
        with open(cns / "maps" / "Unicode" / "CNS2UNICODE_Unicode BMP.txt", encoding="utf-8-sig") as fh:
            for line in fh:
                parts = line.split()
                if len(parts) == 2:
                    code_to_char[parts[0]] = chr(int(parts[1], 16))
        with open(cns / "props" / "CNS_phonetic.txt", encoding="utf-8-sig") as fh:
            for line in fh:
                parts = line.rstrip("\n").split("\t")
                ch = code_to_char.get(parts[0]) if len(parts) == 2 else None
                if ch is None or not ("一" <= ch <= "鿿") or not SYLLABLE.match(parts[1]):
                    continue
                # Simplified forms are left out: this is a Traditional Chinese input method.
                if to_trad.convert(ch) != ch:
                    continue
                lines.append(f"cns\t{ch}\t{parts[1]}")
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"{out}: {len(lines)} lines")


def read_extra(path: Path) -> dict[str, list[list[str]]]:
    extra: dict[str, list[list[str]]] = defaultdict(list)
    for line in path.read_text(encoding="utf-8").splitlines():
        if line:
            kind, *fields = line.split("\t")
            extra[kind].append(fields)
    return extra


def build(moe_dir: Path, extra_path: Path, out_dir: Path) -> None:
    """The dictionary from the MOE tables (as downloaded: unpacked or still zipped) and
    `make_extra`'s file. This step reads MOE content, so it runs on the user's machine."""
    out_dir.mkdir(parents=True, exist_ok=True)
    extra = read_extra(extra_path)
    sources = [
        ("concised", find_moe(moe_dir, "dict_concised_*.xlsx"), "字詞名", "注音一式", "變體注音"),
        ("idiom", find_moe(moe_dir, "dict_idioms_*.xlsx"), "成語", "注音", None),
        ("revised", find_moe(moe_dir, "dict_revised_*.xlsx"), "字詞名", "注音一式", "變體注音"),
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

    for word, bpmf in extra["manual"]:
        add(word, bpmf.split(), "manual")
        stats["manual"] += 1
        if len(word) == 1:  # a single-character line also fixes that character's default
            overrides[word] = bpmf

    need: list[tuple[str, int, str]] = []
    known = {w for w, _ in entries}
    for word, count in extra["twngrams"]:
        if word in known:
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

    # Spellings people type that differ from the dictionary reading (typing_variants.tsv).
    variants: dict[tuple[str, str], str] = {(ch, syl): typed for ch, syl, typed in extra["variant"]}
    typed_added = 0
    for (word, syls), tag in list(entries.items()):
        if not any((ch, syl) in variants for ch, syl in zip(word, syls)):
            continue
        key = (word, tuple(variants.get((ch, syl), syl) for ch, syl in zip(word, syls)))
        if key not in entries:
            entries[key] = tag if tag.endswith(":var") else tag.split(":")[0] + ":alt"
            typed_added += 1
    stats["typing_variants"] = typed_added

    # Characters missing from the dictionaries above, from the CNS 11643 character
    # database (tag "cns"). Only whole characters are added; nothing existing changes.
    # Simplified forms are left out: this is a Traditional Chinese input method.
    have = {w for w, _ in entries if len(w) == 1}
    cns_chars = set()
    for ch, syl in extra["cns"]:
        if ch not in have:
            entries[(ch, (syl,))] = "cns"
            cns_chars.add(ch)
    stats["cns_chars"] = len(cns_chars)

    # Tone sandhi of 一 and 不: people type the tone they say (一個 as ㄧˊ, 一起 as ㄧˋ,
    # 不要 as ㄅㄨˊ). Words get an ":alt" entry with the spoken tone; the single
    # characters get a ":var" entry, which the engine only uses when the next
    # syllable has the tone that causes the change (see SANDHI in the engine's decoder).
    def tone_of(syl: str) -> int:
        return 5 if syl.startswith("˙") else {"ˊ": 2, "ˇ": 3, "ˋ": 4}.get(syl[-1], 1)

    def spoken(ch: str, syl: str, nxt: str) -> str:
        if ch == "不" and syl == "ㄅㄨˋ" and tone_of(nxt) == 4:
            return "ㄅㄨˊ"
        if ch == "一" and syl == "ㄧ":
            if tone_of(nxt) == 4 or nxt == "˙ㄍㄜ":
                return "ㄧˊ"
            if tone_of(nxt) in (1, 2, 3):
                return "ㄧˋ"
        return syl

    sandhi_added = 0
    for (word, syls), tag in list(entries.items()):
        if len(word) < 2 or tag.endswith(":var") or not ("一" in word[:-1] or "不" in word[:-1]):
            continue
        new = tuple(spoken(word[i], syls[i], syls[i + 1]) if i + 1 < len(word) else syls[i]
                    for i in range(len(word)))
        if new != syls and (word, new) not in entries:
            entries[(word, new)] = tag.split(":")[0] + ":alt"
            sandhi_added += 1
    for ch, syl in (("一", "ㄧˊ"), ("一", "ㄧˋ"), ("不", "ㄅㄨˊ")):
        entries.setdefault((ch, (syl,)), "concised:var")
    stats["sandhi_alternatives"] = sandhi_added

    # Bopomofo symbols themselves: a lone symbol typed with the first tone can be
    # chosen from the candidate list (tag "symbol"), as in Microsoft Bopomofo.
    for sym in "ㄅㄆㄇㄈㄉㄊㄋㄌㄍㄎㄏㄐㄑㄒㄓㄔㄕㄖㄗㄘㄙㄧㄨㄩㄚㄛㄜㄝㄞㄟㄠㄡㄢㄣㄤㄥㄦ":
        entries[(sym, (sym,))] = "symbol"

    ordered = sorted(entries.items(), key=lambda kv: (kv[0][0], kv[0][1]))
    with open(out_dir / "dict.tsv", "w", encoding="utf-8") as fh:
        for (word, syls), tag in ordered:
            fh.write(f"{word}\t{' '.join(syls)}\t{tag}\n")
    with open(out_dir / "rejected.tsv", "w", encoding="utf-8") as fh:
        for row in rejected:
            fh.write("\t".join(row) + "\n")
    with open(out_dir / "need_reading.tsv", "w", encoding="utf-8") as fh:
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
    (out_dir / "stats.json").write_text(json.dumps(dict(stats), ensure_ascii=False, indent=1))
    print(json.dumps(dict(stats), ensure_ascii=False, indent=1))



def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    e = sub.add_parser("extra", help="non-MOE sources -> dict_extra.tsv (distributable)")
    e.add_argument("out", type=Path)
    e.add_argument("--twngrams", type=Path)
    e.add_argument("--manual", type=Path)
    e.add_argument("--cns", type=Path,
                   help="CNS 11643 open data folder (props/CNS_phonetic.txt, maps/Unicode/...)")
    b = sub.add_parser("build", help="MOE tables + dict_extra.tsv -> dict.tsv (on the user's machine)")
    b.add_argument("moe_dir", type=Path)
    b.add_argument("extra", type=Path)
    b.add_argument("out_dir", type=Path)
    args = ap.parse_args()
    if args.cmd == "extra":
        make_extra(args.twngrams, args.manual, args.cns, args.out)
    else:
        build(args.moe_dir, args.extra, args.out_dir)


if __name__ == "__main__":
    main()
