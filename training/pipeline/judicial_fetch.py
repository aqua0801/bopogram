"""Download monthly judgment archives (裁判書) from the Judicial Yuan open data platform.

Source: 司法院資料開放平台 https://opendata.judicial.gov.tw/ (category 051)
        Open licence, attribution required. Downloads need a (free) member account.

Usage:
    uv run python pipeline/judicial_fetch.py OUT_DIR 201503 202006 202509

Credentials are read from the repo-root .env (TOML syntax, git-ignored):
    judicial_account = "..."
    judicial_password = "..."

Also fetches the "Delete-Infor" list (judgments withdrawn by the courts) so
later stages can drop them. Resume: finished archives are skipped; partial
downloads go to a .tmp name and are restarted.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
import tomllib
import urllib.request
from pathlib import Path

BASE = "https://opendata.judicial.gov.tw"
USER_AGENT = "Mozilla/5.0 (compatible; research-crawler)"
ENV_FILE = Path(__file__).resolve().parents[2] / ".env"


def get_json(url: str, token: str | None = None, body: dict | None = None):
    headers = {"User-Agent": USER_AGENT}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, headers=headers)
    with urllib.request.urlopen(req, timeout=60) as resp:
        return json.loads(resp.read().decode("utf-8-sig"))


def login() -> str:
    cfg = tomllib.loads(ENV_FILE.read_text(encoding="utf-8"))
    res = get_json(f"{BASE}/api/MemberTokens",
                   body={"memberAccount": cfg["judicial_account"], "pwd": cfg["judicial_password"]})
    if not res.get("token"):
        sys.exit("login failed: " + str(res.get("message")))
    return res["token"]


def download(file_set_id: int, dest: Path, token: str) -> int:
    req = urllib.request.Request(
        f"{BASE}/api/FilesetLists/{file_set_id}/file",
        headers={"User-Agent": USER_AGENT, "Authorization": f"Bearer {token}"},
    )
    tmp = dest.with_name(dest.name + ".tmp")
    n = 0
    with urllib.request.urlopen(req, timeout=120) as resp, open(tmp, "wb") as fh:
        expected = int(resp.headers.get("Content-Length") or 0)
        while chunk := resp.read(1 << 20):
            fh.write(chunk)
            n += len(chunk)
    if expected and n != expected:
        raise IOError(f"short download: {n} of {expected} bytes")
    os.replace(tmp, dest)
    return n


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("months", nargs="+", help="YYYYMM")
    ap.add_argument("--delay", type=float, default=5.0)
    args = ap.parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)

    token = login()
    datasets = get_json(f"{BASE}/data/api/rest/categories/051/resources")
    by_month: dict[str, int] = {}
    delete_id = None
    for ds in datasets:
        title = ds.get("title", "")
        filesets = ds.get("filesets") or []
        if not filesets:
            continue
        if title.startswith("Delete-Infor"):
            delete_id = filesets[0]["fileSetId"]
        elif m := re.match(r"(\d{6})裁判書", title):
            by_month[m.group(1)] = filesets[0]["fileSetId"]

    jobs: list[tuple[int, Path]] = []
    if delete_id:
        jobs.append((delete_id, args.out_dir / "delete_infor.csv"))  # always refreshed
    for month in args.months:
        if month not in by_month:
            print(f"{month}: not in catalogue, skipped", flush=True)
            continue
        dest = args.out_dir / f"{month}.rar"
        if dest.exists():
            print(f"{month}: already downloaded", flush=True)
            continue
        jobs.append((by_month[month], dest))

    for file_set_id, dest in jobs:
        for attempt in range(3):
            try:
                n = download(file_set_id, dest, token)
                print(f"{dest.name}: {n / 1e6:.1f}MB", flush=True)
                break
            except Exception as e:
                print(f"{dest.name}: attempt {attempt + 1} failed: {e!r}", flush=True)
                time.sleep(30)
        time.sleep(args.delay)


if __name__ == "__main__":
    main()
