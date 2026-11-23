"""Turn the approved typing-log records (tools/review_log.py --export) into an
evaluation set in the format of eval/testset, so `bopogram eval` can score them.

Usage (WSL):
    python tools/log_to_eval.py EXPORT_JSONL OUT_DIR

Writes OUT_DIR/personal.tsv (id, category, context, text, note) and
OUT_DIR/personal_bopomofo.tsv (id, bopomofo). Categories:
    corrected  the user changed the engine's choice before committing it
               (the engine of that time got 0% of these)
    accepted   committed as the engine chose it (that engine got 100%)
The note holds the style, how many times the record occurred, and for
corrected records what the engine chose at the time.

The data is personal: keep OUT_DIR outside the repo (default place:
~/bopogram-data/personal/) and never put it in git. Only records that were
approved in the review tool are in the export; this script reads nothing else.
A typed space is written as U+3000 (the engine treats it the same). Records whose
items and text differ in length, or that contain a tab or a newline, are skipped. Then, for example:

    bopogram --config CONFIG eval OUT_DIR/personal.tsv --readings OUT_DIR/personal_bopomofo.tsv --style casual
"""

from __future__ import annotations

import argparse
import json
from collections import Counter
from pathlib import Path


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("export", type=Path)
    ap.add_argument("out_dir", type=Path)
    args = ap.parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)
    rows, skipped, styles = [], 0, Counter()
    with open(args.export, encoding="utf-8") as fh:
        for line in fh:
            r = json.loads(line)
            # A typed space becomes U+3000: the files are space-separated, and the
            # engine treats both as a space (not scored by the model).
            sp = lambda x: x.replace(" ", "\u3000")
            items, text, first = [sp(i) for i in r["items"]], sp(r["text"]), sp(r["first"])
            r["context"] = sp(r.get("context") or "")
            fields = [text, first, r["context"], *items]
            if len(items) != len(text) or any(c in f for f in fields for c in "\t\n") \
                    or all(i.startswith("=") for i in items):
                skipped += 1
                continue
            corrected = first != text
            note = f"style={r.get('style', '')} count={r.get('count', 1)}" + (f" first={first}" if corrected else "")
            rows.append(("corrected" if corrected else "accepted", r.get("context") or "", text, note, items))
            styles[r.get("style", "")] += 1
    with open(args.out_dir / "personal.tsv", "w", encoding="utf-8") as t, \
            open(args.out_dir / "personal_bopomofo.tsv", "w", encoding="utf-8") as b:
        t.write("id\tcategory\tcontext\ttext\tnote\n")
        b.write("id\tbopomofo\n")
        for n, (cat, ctx, text, note, items) in enumerate(rows, 1):
            rid = f"P{n:05d}"
            t.write(f"{rid}\t{cat}\t{ctx}\t{text}\t{note}\n")
            b.write(f"{rid}\t{' '.join(items)}\n")
    cats = Counter(r[0] for r in rows)
    print(f"{len(rows)} records ({cats['corrected']} corrected, {cats['accepted']} accepted), "
          f"{skipped} skipped; styles: {dict(styles)}")


if __name__ == "__main__":
    main()
