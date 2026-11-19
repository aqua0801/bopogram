"""Extract Traditional Chinese, non-Hong Kong web pages from FineWeb2 (cmn_Hani).

Source: HuggingFaceFW/fineweb-2 (ODC-By 1.0, also subject to the Common Crawl Terms of Use)
        https://huggingface.co/datasets/HuggingFaceFW/fineweb-2

Usage:
    uv run python pipeline/fineweb_extract.py OUT_DIR --files 0-4 [--workers 12]

The Chinese part is 370 parquet files of about 4.4 GB (5 groups, 000_00000 ...
004_00073). --files numbers positions in the sorted list. Each selected file is
downloaded to OUT_DIR/download/, filtered row group by row group in parallel,
written as OUT_DIR/part-fineweb-GGG_NNNNN.jsonl.gz and then deleted, so only one or
two files are on disk at a time. The next file downloads while the current one
is filtered. Finished files are skipped on restart.

A page is kept when
  - its text is Traditional Chinese: converting it to Simplified changes far
    more characters than converting it to Traditional does,
  - its host is not under .hk and the text has few Cantonese words,
  - its host is not in data/fineweb_exclude.txt (sites that forbid AI training).
Output: {"id","source","title","text","url"}; clean.py ignores the url, the
domain report (fineweb_domains.py) reads it.
"""

from __future__ import annotations

import argparse
import gzip
import json
import os
import re
import threading
from collections import Counter
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path
from urllib.parse import urlsplit

REPO = "HuggingFaceFW/fineweb-2"
REMOTE_DIR = "data/cmn_Hani/train"
EXCLUDE_FILE = Path(__file__).parent / "data" / "fineweb_exclude.txt"
SAMPLE = 2000          # characters examined for the script test
MIN_CHANGED = 5        # at least this many characters must differ from Simplified
RATIO = 3              # and this many times more than differ from Traditional
CANTONESE = re.compile(r"[嘅咗哋冇喺啲嚟咁嘢佢]|唔係|唔好|乜嘢")
MAX_CANTONESE = 3
# Traces of Simplified text run through a converter: variant forms Taiwan does not
# use, and typical one-to-many mistakes (了->瞭, 后->後 or kept, 里->裏, 干->乾 ...).
# On sample sites this dropped 54% of pages from converted-content sites and 5%
# of pages from Taiwanese ones.
CONVERTED = re.compile(r"[爲説衆啓着裏綫峯羣牀僞鷄麪衹]|(?<![明])瞭(?![解望若然])|(?<![皇王影天母太])后"
                       r"|皇後|王後|阿裏|乾部|頭發|理發")
MAX_CONVERTED = 0.5    # per 1,000 characters
MIN_CONVERTED = 3      # and at least this many in the page (one stray 説 is not a converter)

_t2s = _s2t = None


def load_exclude() -> tuple[str, ...]:
    return tuple(line.strip() for line in EXCLUDE_FILE.read_text(encoding="utf-8").splitlines()
                 if line.strip() and not line.startswith("#"))


def excluded(host: str, domains: tuple[str, ...]) -> bool:
    return any(host == d or host.endswith("." + d) for d in domains)


def classify(text: str, url: str, domains: tuple[str, ...]) -> str:
    """'keep' or the reason the page is dropped."""
    global _t2s, _s2t
    if _t2s is None:
        import opencc
        _t2s, _s2t = opencc.OpenCC("t2s"), opencc.OpenCC("s2t")
    host = (urlsplit(url).hostname or "").lower()
    if excluded(host, domains):
        return "excluded_site"
    if host.endswith(".hk"):
        return "hk_domain"
    s = text[:SAMPLE]
    to_simp = sum(a != b for a, b in zip(s, _t2s.convert(s)))
    to_trad = sum(a != b for a, b in zip(s, _s2t.convert(s)))
    if to_simp < MIN_CHANGED or to_simp < RATIO * to_trad:
        return "simplified" if to_trad >= MIN_CHANGED and to_trad >= RATIO * to_simp else "unclear"
    if len(CANTONESE.findall(text)) > MAX_CANTONESE:
        return "cantonese"
    hits = len(CONVERTED.findall(text))
    if hits >= MIN_CONVERTED and hits * 1000 > MAX_CONVERTED * len(text):
        return "converted"
    return "keep"


def refilter(path: Path, domains: tuple[str, ...]) -> Counter:
    """Apply the current rules (e.g. a longer exclusion list) to an existing output file."""
    stats: Counter = Counter()
    tmp = path.with_suffix(".tmp")
    with gzip.open(path, "rt", encoding="utf-8") as fin, \
            gzip.open(tmp, "wt", encoding="utf-8", compresslevel=5) as fout:
        for line in fin:
            doc = json.loads(line)
            why = classify(doc["text"], doc["url"], domains)
            stats[why] += 1
            if why == "keep":
                stats["kept_chars"] += len(doc["text"])
                fout.write(line)
    os.replace(tmp, path)
    return stats


def filter_group(path: str, group: int, domains: tuple[str, ...]) -> tuple[list[str], Counter]:
    import pyarrow.parquet as pq
    table = pq.ParquetFile(path).read_row_group(group, columns=["id", "text", "url"])
    lines, stats = [], Counter()
    for doc_id, text, url in zip(*(table.column(c).to_pylist() for c in ("id", "text", "url"))):
        why = classify(text, url, domains)
        stats[why] += 1
        if why == "keep":
            stats["kept_chars"] += len(text)
            lines.append(json.dumps({"id": doc_id, "source": "fineweb2", "title": "", "text": text,
                                     "url": url}, ensure_ascii=False))
    return lines, stats


def remote_files() -> list[str]:
    from huggingface_hub import HfApi
    return sorted(f.path for f in HfApi().list_repo_tree(REPO, repo_type="dataset", path_in_repo=REMOTE_DIR))


def download(remote: str, dest: Path) -> Path:
    from huggingface_hub import hf_hub_download
    return Path(hf_hub_download(REPO, remote, repo_type="dataset", local_dir=dest))


def out_name(remote: str) -> str:
    return "part-fineweb-" + remote.rsplit("/", 1)[1].removesuffix(".parquet")


def process(path: Path, name: str, out_dir: Path, pool, domains) -> Counter:
    import pyarrow.parquet as pq
    groups = pq.ParquetFile(path).metadata.num_row_groups
    stats: Counter = Counter()
    tmp = out_dir / f"{name}.tmp"
    with gzip.open(tmp, "wt", encoding="utf-8", compresslevel=5) as fh:
        # map keeps row-group order, so output is deterministic
        for lines, st in pool.map(filter_group, [str(path)] * groups, range(groups), [domains] * groups,
                                  chunksize=4):
            stats.update(st)
            for line in lines:
                fh.write(line + "\n")
    (out_dir / f"{name}.stats.json").write_text(json.dumps(stats))
    os.replace(tmp, out_dir / f"{name}.jsonl.gz")  # last: marks the file done
    return stats


def parse_files(spec: str) -> list[int]:
    out = []
    for part in spec.split(","):
        a, _, b = part.partition("-")
        out.extend(range(int(a), int(b or a) + 1))
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--files", required=True, help="positions in the sorted file list, e.g. 0-4 or 0,74,148")
    ap.add_argument("--workers", type=int, default=12)
    ap.add_argument("--refilter", action="store_true",
                    help="only re-apply the current rules to these files' finished output")
    args = ap.parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)
    dl_dir = args.out_dir / "download"
    domains = load_exclude()
    remotes = remote_files()
    chosen = [remotes[i] for i in parse_files(args.files)]
    if args.refilter:
        paths = [p for r in chosen if (p := args.out_dir / f"{out_name(r)}.jsonl.gz").exists()]
        with ProcessPoolExecutor(args.workers) as pool:
            for p, st in zip(paths, pool.map(refilter, paths, [domains] * len(paths))):
                print(f"{p.name}: kept {st['keep']}, {st['kept_chars'] / 1e6:.0f}M characters; "
                      + ", ".join(f"{k} {v}" for k, v in st.most_common() if k not in ("keep", "kept_chars")),
                      flush=True)
        return
    todo = [r for r in chosen if not (args.out_dir / f"{out_name(r)}.jsonl.gz").exists()]
    print(f"{len(todo)} files to process, excluding {len(domains)} domains", flush=True)

    def fetch(i: str, box: dict) -> None:
        try:
            box["path"] = download(i, dl_dir)
        except BaseException as e:  # reported by the main thread
            box["error"] = e

    with ProcessPoolExecutor(args.workers) as pool:
        nxt = None
        for k, i in enumerate(todo):
            if nxt is None:
                box: dict = {}
                fetch(i, box)
            else:
                thread, box = nxt
                thread.join()
            if "error" in box:
                raise box["error"]
            nxt = None
            if k + 1 < len(todo):
                b2: dict = {}
                t2 = threading.Thread(target=fetch, args=(todo[k + 1], b2))
                t2.start()
                nxt = (t2, b2)
            st = process(box["path"], out_name(i), args.out_dir, pool, domains)
            box["path"].unlink()
            n = sum(v for key, v in st.items() if key != "kept_chars")
            print(f"{out_name(i)}: kept {st['keep']}/{n} pages, {st['kept_chars'] / 1e6:.0f}M characters; "
                  + ", ".join(f"{key} {v}" for key, v in st.most_common() if key not in ("keep", "kept_chars")),
                  flush=True)


if __name__ == "__main__":
    main()
