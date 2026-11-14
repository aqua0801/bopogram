"""Test-set evaluation: top-1 accuracy, baselines, and per-keystroke latency.

Systems:
  bopogram      the full engine (lattice + n-gram mixture + left context)
  word-unigram  same lattice, but words are scored without any context
  char-unigram  each syllable becomes its most frequent character

Latency is measured the way an input method is used: after every syllable
the lattice is extended and the best result is read back.
"""

from __future__ import annotations

import csv
import json
import statistics
import time
from collections import defaultdict

OVERLAP_MARK = "〔語料重複〕"
# Third-person pronouns sound identical; which one is meant often cannot be
# known from the sentence. A second score treats them as interchangeable.
_PRONOUNS = str.maketrans({"她": "他", "它": "他", "牠": "他", "祂": "他"})


class _UnigramLM:
    """Wraps a MixtureLM so that every word is scored without context."""

    def __init__(self, lm):
        self._lm = lm
        self._state = ("unigram",)

    def begin(self):
        return self._state

    def score(self, state, token):
        return self._lm.unigram(token), self._state

    def unigram(self, token):
        return self._lm.unigram(token)

    def map_token(self, tok, in_vocab):
        return self._lm.map_token(tok, in_vocab)


def load_testset(path: str) -> list[dict]:
    with open(path, encoding="utf-8", newline="") as fh:
        return list(csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE))


def load_readings(path: str) -> dict[str, list[str]]:
    out = {}
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            parts = line.rstrip("\n").split("\t")
            if len(parts) >= 2 and parts[0] != "id":
                out[parts[0]] = parts[1].split()
    return out


def percentile(values: list[float], q: float) -> float:
    if not values:
        return 0.0
    values = sorted(values)
    k = min(len(values) - 1, max(0, round(q * (len(values) - 1))))
    return values[k]


def run_eval(args) -> int:
    from .context import context_state
    from .decoder import Decoder
    from .engine import Engine

    rows = load_testset(args.testset)
    readings = load_readings(args.readings)
    engine = Engine(args.config, use_user_model=False)

    char_freq: dict[str, float] = {}
    if args.system == "char-unigram":
        if not args.char_freq:
            raise SystemExit("--char-freq is required for the char-unigram baseline")
        with open(args.char_freq, encoding="utf-8") as fh:
            for line in fh:
                ch, _, n = line.rstrip("\n").partition("\t")
                char_freq[ch] = float(n)
    if args.system == "word-unigram":
        decoder = Decoder(engine.lexicon, _UnigramLM(engine.lm), beam=1,
                          max_candidates=engine.decoder.max_candidates)
    else:
        decoder = engine.decoder

    def convert(syllables: list[str], context: str, timings: list[float]) -> str:
        if args.system == "char-unigram":
            out = []
            for syl in syllables:
                t0 = time.perf_counter()
                cands = [w for w in engine.lexicon.candidates((syl,)) if len(w) == 1]
                out.append(max(cands, key=lambda c: char_freq.get(c, 0.0)) if cands else "?")
                timings.append(time.perf_counter() - t0)
            return "".join(out)
        if args.system == "word-unigram":
            decoder.reset()
        else:
            decoder.reset(context_state(context, engine.lexicon, engine.lm))
        text = ""
        for syl in syllables:
            t0 = time.perf_counter()
            decoder.push(syl)
            text = decoder.best_text()
            timings.append(time.perf_counter() - t0)
        return text

    # Warm up caches so the first sentences do not distort latency.
    for row in rows[:20]:
        if row["id"] in readings:
            convert(readings[row["id"]], row.get("context") or "", [])

    stats = defaultdict(lambda: defaultdict(float))
    timings: list[float] = []
    errors: list[str] = []
    skipped = 0
    for row in rows:
        syls = readings.get(row["id"])
        gold = row["text"]
        if not syls or len(syls) != len(gold):
            skipped += 1
            continue
        hyp = convert(syls, row.get("context") or "", timings)
        correct_chars = sum(1 for a, b in zip(hyp, gold) if a == b) if len(hyp) == len(gold) else 0
        groups = ["all", row["category"]]
        if OVERLAP_MARK not in (row.get("note") or ""):
            groups.append("all_excluding_overlap")
        for g in groups:
            s = stats[g]
            s["sentences"] += 1
            s["sentences_correct"] += hyp == gold
            s["sentences_correct_pron"] += hyp.translate(_PRONOUNS) == gold.translate(_PRONOUNS)
            s["chars"] += len(gold)
            s["chars_correct"] += correct_chars
        if hyp != gold:
            errors.append(f"{row['id']}\t{row['category']}\t{gold}\t{hyp}\t{row.get('note') or ''}")

    result = {"system": args.system, "skipped": skipped, "groups": {}}
    print(f"system: {args.system}   (skipped {skipped} rows without usable readings)")
    print(f"{'group':<24}{'sentences':>10}{'sent.acc':>10}{'char.acc':>10}{'sent.acc(pronouns merged)':>28}")
    for g in sorted(stats, key=lambda k: (not k.startswith("all"), k)):
        s = stats[g]
        sent_acc = s["sentences_correct"] / s["sentences"]
        char_acc = s["chars_correct"] / s["chars"]
        pron_acc = s["sentences_correct_pron"] / s["sentences"]
        result["groups"][g] = {"sentences": int(s["sentences"]), "sentence_accuracy": round(sent_acc, 4),
                               "char_accuracy": round(char_acc, 4),
                               "sentence_accuracy_pronouns_merged": round(pron_acc, 4)}
        print(f"{g:<24}{int(s['sentences']):>10}{sent_acc:>10.1%}{char_acc:>10.1%}{pron_acc:>28.1%}")
    ms = [t * 1000 for t in timings]
    result["latency_ms"] = {"p50": round(percentile(ms, 0.5), 3), "p95": round(percentile(ms, 0.95), 3),
                            "mean": round(statistics.fmean(ms), 3) if ms else 0.0, "keystrokes": len(ms)}
    print(f"latency per syllable: P50 {result['latency_ms']['p50']:.2f} ms, "
          f"P95 {result['latency_ms']['p95']:.2f} ms over {len(ms)} syllables")
    if args.errors:
        with open(args.errors, "w", encoding="utf-8") as fh:
            fh.write("id\tcategory\tgold\toutput\tnote\n" + "\n".join(errors) + "\n")
    if args.json:
        with open(args.json, "w", encoding="utf-8") as fh:
            json.dump(result, fh, ensure_ascii=False, indent=1)
    return 0
