"""Test-set evaluation: top-1 accuracy, baselines, and per-keystroke latency.

Systems:
  bopogram      the full engine (lattice + n-gram mixture + left context)
  word-unigram  same lattice, but words are scored without any context
  char-unigram  each syllable becomes its most frequent character

Latency is measured the way an input method is used: after every syllable
the lattice is extended and the best result is read back.

With --neural-scores the per-position scores of the neural model (computed
beforehand by training/neural/score_testset.py) are combined with the n-gram
engine; latency is not measured in that case, because the model is not run here.

Scoring: where the output uses the other standard form of a character or word
(臺/台, 記錄/紀錄, ...; VARIANT_CHARS, VARIANT_WORDS), it counts as right.
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
# Both forms are standard in Taiwan (Ministry of Education), so writing either is
# not an error: single characters, and two-character words for pairs that are
# only interchangeable inside those words.
VARIANT_CHARS = [frozenset(p) for p in
                 ("臺台", "裏裡", "祕秘", "週周", "佈布", "姊姐", "佔占", "羣群", "汙污", "菸煙", "豔艷", "歎嘆")]
VARIANT_WORDS = [frozenset(p) for p in
                 (("記錄", "紀錄"), ("作法", "做法"), ("作夢", "做夢"), ("作為", "做為"), ("身分", "身份"),
                  ("部分", "部份"), ("甚麼", "什麼"), ("計畫", "計劃"), ("念書", "唸書"))]


def accept_variants(gold: str, hyp: str) -> str:
    """`gold` with every character where `hyp` uses the other standard form replaced by
    `hyp`'s, so that such a difference does not count as an error."""
    if len(gold) != len(hyp):
        return gold
    out = list(gold)
    for i, (g, h) in enumerate(zip(gold, hyp)):
        if g == h:
            continue
        if frozenset((g, h)) in VARIANT_CHARS or any(
                k >= 0 and frozenset((gold[k:k + 2], hyp[k:k + 2])) in VARIANT_WORDS for k in (i - 1, i)):
            out[i] = h
    return "".join(out)


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
                # ASCII spaces only: a literal full-width space ("=\u3000") is an item.
                out[parts[0]] = [p for p in parts[1].split(" ") if p]
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
    engine.set_style(args.style)
    if args.neural_model:
        engine.set_neural(args.neural_model, args.neural_weight, args.neural_floor)
    if args.charlm_model:
        engine.set_charlm(args.charlm_model, args.charlm_weight, args.charlm_top)
    if args.no_charlm:
        engine.charlm = None
        engine.decoder.merge = True
    live = ((engine.neural is not None or engine.charlm is not None)
            and args.system == "bopogram" and not args.neural_scores)

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
                          max_candidates=engine.decoder.max_candidates, style=args.style)
    else:
        decoder = engine.decoder

    neural: dict[str, dict] = {}
    if args.neural_scores:
        with open(args.neural_scores, encoding="utf-8") as fh:
            for line in fh:
                rec = json.loads(line)
                neural[rec["id"]] = rec

    def neural_sum(scores: list[dict[str, float]], text: str) -> float:
        return sum(max(s.get(ch, args.neural_floor), args.neural_floor) for s, ch in zip(scores, text))

    def convert(syllables: list[str], context: str, timings: list[float], row_id: str = "") -> str:
        rec = neural.get(row_id)
        if rec is not None and args.system == "bopogram":
            if args.neural_mode == "alone":
                return rec["best"]
            decoder.reset(context_state(context, engine.lexicon, engine.lm))
            if args.neural_mode == "fuse":
                decoder.set_position_scores(rec["scores"], args.neural_weight, args.neural_floor)
            for syl in syllables:
                decoder.push(syl)
            decoder.set_position_scores(None)
            if args.neural_mode == "fuse":
                return decoder.best_text()
            ranked = [("".join(words), score) for words, score in decoder.nbest(10)]
            return max(ranked, key=lambda r: r[1] + args.neural_weight * neural_sum(rec["scores"], r[0]))[0]
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
            if live:
                engine.rescore(decoder, context)
            text = decoder.best_text()
            timings.append(time.perf_counter() - t0)
        if live:
            decoder.set_position_scores(None)
        return text

    # Warm up caches so the first sentences do not distort latency.
    for row in rows[:20]:
        if row["id"] in readings:
            convert(readings[row["id"]], row.get("context") or "", [])

    if args.dump_nbest and args.beam:
        decoder.beam, decoder.merge = args.beam, False
    dump = open(args.dump_nbest, "w", encoding="utf-8") if args.dump_nbest and args.system == "bopogram" else None
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
        hyp = convert(syls, row.get("context") or "", timings, row["id"])
        if dump is not None:
            hyps = [["".join(words), round(score, 4)] for words, score in decoder.nbest(args.nbest)]
            dump.write(json.dumps({"id": row["id"], "category": row["category"], "context": row.get("context") or "",
                                   "gold": gold, "hyps": hyps}, ensure_ascii=False) + "\n")
        if args.style == "casual":
            # Casual style writes 的 for every structural particle; that is not an error.
            gold = "".join("的" if s == "˙ㄉㄜ" and g in "地得" and h == "的" else g
                           for g, h, s in zip(gold, hyp, syls))
        gold = accept_variants(gold, hyp)
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

    if dump is not None:
        dump.close()
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
