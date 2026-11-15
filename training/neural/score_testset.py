"""Score test sentences with a trained model, for evaluation in the prototype engine.

Usage:
    uv run python neural/score_testset.py CHECKPOINT DATA_DIR TESTSET_TSV READINGS_TSV OUT_JSONL [--toneless]

Output, one line per sentence:
    {"id": "H001", "best": "...", "scores": [{"字": log p, ...}, ...]}
`scores[i]` holds the log-probability of every candidate character of syllable
i, given the left context and all syllables of the sentence. The prototype
engine adds these to its lattice (`bopogram eval --neural-scores`), which keeps
PyTorch out of the engine's environment. `best` is the model's own choice.
Prints the accuracy of the model used alone.
"""

from __future__ import annotations

import argparse
import csv
import json
import sys
from pathlib import Path

import torch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from model import TONE_UNKNOWN, Config, Converter, Tables  # noqa: E402

MAX_CONTEXT = 40


def load_model(ckpt_path: Path, data_dir: Path, device):
    vocab = json.loads((data_dir / "vocab.json").read_text(encoding="utf-8"))
    tables = Tables(vocab)
    ckpt = torch.load(ckpt_path, map_location=device, weights_only=False)
    model = Converter(Config(**ckpt["config"])).to(device)
    model.load_state_dict(ckpt["model"])
    model.eval()
    return model, tables, ckpt["step"]


def context_ids(text: str, tables: Tables) -> list[int]:
    """Left context -> character ids, the way make_data.py encodes text."""
    cid, out, run = tables.char_id, [], None
    for ch in text[-MAX_CONTEXT:]:
        kind = "<n>" if ch.isdigit() else "<e>" if ch.isascii() and ch.isalpha() else None
        if kind:
            if run != kind:
                out.append(cid[kind])
            run = kind
            continue
        run = None
        if ch.isspace():
            continue
        out.append(cid.get(ch, cid["<x>"] if "一" <= ch <= "鿿" else cid["<p>"]))
    return out


@torch.no_grad()
def score(model, tables: Tables, context: str, syllables: list[str], device, toneless: bool = False):
    """-> list (one per syllable) of {character: log probability}; None if a syllable is unknown."""
    try:
        syl = [tables.syl_id[s] for s in syllables]
    except KeyError:
        return None
    ctx = context_ids(context, tables)
    syls = torch.tensor([[0] * len(ctx) + syl], device=device)
    chars = torch.tensor([ctx + [tables.mask_char] * len(syl)], device=device)
    bases = tables.syl_base.to(device)[syls]
    tones = tables.syl_tone.to(device)[syls]
    if toneless:
        tones = torch.where(syls > 0, torch.full_like(tones, TONE_UNKNOWN), tones)
    hidden = model(chars, bases, tones, torch.tensor([chars.shape[1]], device=device))
    logits = model.logits(hidden[0, len(ctx):]).float().cpu()
    out = []
    for i, s in enumerate(syl):
        allowed = (tables.allowed_base[tables.syl_base[s]] if toneless else tables.allowed[s]).nonzero()[:, 0]
        logp = torch.log_softmax(logits[i, allowed], dim=-1)
        out.append({tables.chars[int(c)]: round(float(p), 4) for c, p in zip(allowed, logp)})
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("checkpoint", type=Path)
    ap.add_argument("data_dir", type=Path)
    ap.add_argument("testset", type=Path)
    ap.add_argument("readings", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--toneless", action="store_true", help="withhold all tones")
    ap.add_argument("--cpu", action="store_true")
    args = ap.parse_args()

    device = torch.device("cpu" if args.cpu or not torch.cuda.is_available() else "cuda")
    model, tables, step = load_model(args.checkpoint, args.data_dir, device)
    readings = {}
    for line in args.readings.read_text(encoding="utf-8").splitlines():
        parts = line.split("\t")
        if len(parts) >= 2 and parts[0] != "id":
            readings[parts[0]] = parts[1].split()
    with open(args.testset, encoding="utf-8", newline="") as fh:
        rows = list(csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE))

    by_cat: dict[str, list[int]] = {}
    tmp = args.out.with_suffix(".tmp")
    with open(tmp, "w", encoding="utf-8") as out:
        for row in rows:
            syls = readings.get(row["id"])
            if not syls or len(syls) != len(row["text"]):
                continue
            scores = score(model, tables, row.get("context") or "", syls, device, args.toneless)
            if scores is None:
                continue
            best = "".join(max(s, key=s.get) for s in scores)
            out.write(json.dumps({"id": row["id"], "best": best, "scores": scores}, ensure_ascii=False) + "\n")
            for group in ("all", row["category"]):
                st = by_cat.setdefault(group, [0, 0, 0, 0])
                st[0] += 1
                st[1] += best == row["text"]
                st[2] += len(best)
                st[3] += sum(a == b for a, b in zip(best, row["text"]))
    tmp.replace(args.out)
    print(f"model alone (step {step}{', toneless' if args.toneless else ''})")
    for group, (n, ok, chars, chars_ok) in sorted(by_cat.items(), key=lambda kv: (kv[0] != "all", kv[0])):
        print(f"{group:<12}{n:>6}  sentence {ok / n:6.1%}  character {chars_ok / chars:6.1%}")


if __name__ == "__main__":
    main()
