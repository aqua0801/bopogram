"""Build the final training corpus from cleaned sources.

Usage:
    uv run python pipeline/mix.py CLEAN_ROOT CORPUS_DIR [--max-cn-density 2.0]
                                  [--cap judicial=0.15] [--replan]

Input : CLEAN_ROOT/<source>/part-*.jsonl.gz   (from clean.py)
Output: CORPUS_DIR/<source>/part-*.txt.gz     one sentence per line
        CORPUS_DIR/plan.json                  rules, per-source counts, sampling rates

Document-level rules (a document is kept or dropped as a whole):
  1. cn_density >= --max-cn-density  -> dropped (China-usage heavy).
  2. Template residue: if >= 30% of a document's sentences are short sentences
     that repeat >= 1000 times corpus-wide (bot-generated stubs), it is dropped.
  3. Capped sources (--cap NAME=FRACTION) are down-sampled so the source makes
     up at most FRACTION of all kept characters. Sampling is deterministic
     (hash of the document id), so reruns select the same documents.

Streaming / resume: pass 1 streams every shard once to count and writes
plan.json; pass 2 streams again and writes one output shard per input shard
atomically. With an existing plan.json pass 1 is skipped (use --replan after
the inputs change) and finished output shards are skipped.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
import re
import shutil
from collections import Counter
from pathlib import Path

SHORT = 12            # only sentences this short are tracked as possible template residue
REPEAT_MIN = 1000     # ...and count as residue when repeated at least this often
RESIDUE_RATIO = 0.3
_RE_CJK = re.compile(r"[一-鿿㐀-䶿]")


def iter_docs(src_dir: Path):
    for shard in sorted(src_dir.glob("part-*.jsonl.gz")):
        with gzip.open(shard, "rt", encoding="utf-8") as fh:
            for line in fh:
                yield shard, json.loads(line)


def is_residue(sents: list[str], frequent: set[str]) -> bool:
    hits = sum(1 for s in sents if s in frequent)
    return hits >= RESIDUE_RATIO * len(sents)


def keep_fraction(doc_id: str, source: str) -> float:
    h = hashlib.blake2b(f"{source}/{doc_id}".encode(), digest_size=8).digest()
    return int.from_bytes(h, "little") / 2**64


def cjk_len(sents: list[str]) -> int:
    return sum(len(_RE_CJK.findall(s)) for s in sents)


def make_plan(clean_root: Path, max_density: float, caps: dict[str, float]) -> dict:
    sources = sorted(p.name for p in clean_root.iterdir() if p.is_dir())
    freq: Counter = Counter()
    for src in sources:
        for _, doc in iter_docs(clean_root / src):
            freq.update(s for s in doc["sents"] if len(s) <= SHORT)
    frequent = sorted(s for s, n in freq.items() if n >= REPEAT_MIN)
    del freq
    fset = set(frequent)

    stats = {s: Counter() for s in sources}
    for src in sources:
        st = stats[src]
        for _, doc in iter_docs(clean_root / src):
            st["docs"] += 1
            if doc["cn_density"] >= max_density:
                st["dropped_cn"] += 1
            elif is_residue(doc["sents"], fset):
                st["dropped_residue"] += 1
            else:
                st["eligible_docs"] += 1
                st["eligible_chars"] += cjk_len(doc["sents"])
        print(f"planned {src}: {dict(st)}", flush=True)

    # Solve sampling rates so each capped source is <= its fraction of the total.
    rates = {s: 1.0 for s in sources}
    uncapped = sum(stats[s]["eligible_chars"] for s in sources if s not in caps)
    cap_total = sum(caps.get(s, 0) for s in sources if s in caps)
    for s in sources:
        if s in caps and stats[s]["eligible_chars"]:
            # capped_s = f_s * total, total = uncapped / (1 - sum(f))
            allowed = caps[s] * uncapped / max(1e-9, 1 - cap_total)
            rates[s] = min(1.0, allowed / stats[s]["eligible_chars"])
    return {"max_cn_density": max_density, "caps": caps, "rates": rates,
            "stats": {s: dict(c) for s, c in stats.items()}, "frequent": frequent}


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("clean_root", type=Path)
    ap.add_argument("corpus_dir", type=Path)
    ap.add_argument("--max-cn-density", type=float, default=2.0)
    ap.add_argument("--cap", action="append", default=[], metavar="SOURCE=FRACTION")
    ap.add_argument("--replan", action="store_true", help="recount and rewrite all output")
    args = ap.parse_args()
    caps = {k: float(v) for k, v in (c.split("=") for c in args.cap)}

    args.corpus_dir.mkdir(parents=True, exist_ok=True)
    plan_file = args.corpus_dir / "plan.json"
    if args.replan or not plan_file.exists():
        for sub in args.corpus_dir.iterdir():
            if sub.is_dir():
                shutil.rmtree(sub)
        plan = make_plan(args.clean_root, args.max_cn_density, caps)
        plan_file.write_text(json.dumps(plan, ensure_ascii=False))
    else:
        plan = json.loads(plan_file.read_text())
    fset = set(plan["frequent"])
    print("sampling rates:", {k: round(v, 4) for k, v in plan["rates"].items()}, flush=True)

    totals = {}
    for src, rate in plan["rates"].items():
        out_dir = args.corpus_dir / src
        out_dir.mkdir(exist_ok=True)
        kept_docs = kept_sents = kept_chars = 0
        for shard in sorted((args.clean_root / src).glob("part-*.jsonl.gz")):
            stem = shard.name.removesuffix(".jsonl.gz")
            final = out_dir / f"{stem}.txt.gz"
            count_file = out_dir / f"{stem}.count.json"
            if not final.exists():
                tmp = out_dir / f"{stem}.tmp"
                d = s = c = 0
                with gzip.open(shard, "rt", encoding="utf-8") as fin, \
                        gzip.open(tmp, "wt", encoding="utf-8", compresslevel=5) as fout:
                    for line in fin:
                        doc = json.loads(line)
                        if doc["cn_density"] >= plan["max_cn_density"]:
                            continue
                        if is_residue(doc["sents"], fset):
                            continue
                        if rate < 1.0 and keep_fraction(str(doc["id"]), src) >= rate:
                            continue
                        d += 1
                        s += len(doc["sents"])
                        c += cjk_len(doc["sents"])
                        fout.write("\n".join(doc["sents"]) + "\n")
                count_file.write_text(json.dumps({"docs": d, "sents": s, "chars": c}))
                os.replace(tmp, final)
            cnt = json.loads(count_file.read_text())
            kept_docs += cnt["docs"]
            kept_sents += cnt["sents"]
            kept_chars += cnt["chars"]
        totals[src] = {"docs": kept_docs, "sentences": kept_sents, "cjk_chars": kept_chars}
        print(f"{src}: {totals[src]}", flush=True)

    grand = sum(t["cjk_chars"] for t in totals.values()) or 1
    for t in totals.values():
        t["share"] = round(t["cjk_chars"] / grand, 4)
    (args.corpus_dir / "totals.json").write_text(json.dumps(totals, ensure_ascii=False, indent=1))
    print(f"total {grand:,} CJK chars (~{grand * 3 / 1e9:.2f} GB)")


if __name__ == "__main__":
    main()
