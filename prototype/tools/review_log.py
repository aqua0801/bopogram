"""Review the typing log record by record and export only what was approved.

Usage (needs nothing but Python; run it on the machine that has the log):
    python tools/review_log.py USERDATA_DIR                 open the review window
    python tools/review_log.py USERDATA_DIR --stats         import new records, print counts
    python tools/review_log.py USERDATA_DIR --export FILE   write approved records to FILE

USERDATA_DIR is the engine's `userdata` folder. Files used there:
    typing_log.jsonl        written by the engine (see bopogram/typing_log.py)
    typing_review.sqlite    this tool's copy of the records and your decisions
    typing_export.jsonl     default export file

Nothing leaves the folder by itself. The export contains only records you
approved, without timestamps, and without the left context unless you approved
the record together with its context. Copy the export file to the training
machine by hand.

Export format, one JSON object per line:
    {"items": [...], "first": "...", "text": "...", "picked": [[start, length, word], ...],
     "context": "...", "style": "casual", "count": 3}
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sqlite3
from pathlib import Path

LOG_NAME, DB_NAME, EXPORT_NAME = "typing_log.jsonl", "typing_review.sqlite", "typing_export.jsonl"
PENDING, APPROVED, APPROVED_NO_CONTEXT, REJECTED = "pending", "approved", "approved_noctx", "rejected"
FILTERS = [  # label, SQL condition
    ("待審：選字有修改的", "status = 'pending' AND first <> text"),
    ("待審：全部", "status = 'pending'"),
    ("已核准", "status IN ('approved', 'approved_noctx')"),
    ("已拒絕", "status = 'rejected'"),
    ("全部", "1 = 1"),
]


class Store:
    """The review database: one row per distinct record, with a decision."""

    def __init__(self, userdata: Path):
        self.userdata = userdata
        self.log_path = userdata / LOG_NAME
        self.db = sqlite3.connect(userdata / DB_NAME)
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS records (
                id INTEGER PRIMARY KEY, hash TEXT UNIQUE, t TEXT, items TEXT, first TEXT, text TEXT,
                picked TEXT, context TEXT, style TEXT, auto INTEGER, count INTEGER DEFAULT 1,
                status TEXT DEFAULT 'pending');
            CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
        """)

    # -- import -------------------------------------------------------------
    def _offset(self) -> int:
        row = self.db.execute("SELECT value FROM meta WHERE key = 'offset'").fetchone()
        return int(row[0]) if row else 0

    def import_new(self) -> int:
        """Read the lines the engine appended since the last import. Returns how many were read.
        Identical records (same input, same result, same context) are stored once with a count."""
        if not self.log_path.exists():
            return 0
        offset = self._offset()
        if self.log_path.stat().st_size < offset:
            offset = 0  # the log was emptied or replaced
        read = 0
        with open(self.log_path, "rb") as fh:
            fh.seek(offset)
            for raw in fh:
                if not raw.endswith(b"\n"):
                    break  # a line still being written
                offset += len(raw)
                try:
                    rec = json.loads(raw.decode("utf-8"))
                    items, text = rec["items"], rec["text"]
                except (ValueError, KeyError, UnicodeDecodeError):
                    continue
                first, context = rec.get("first", text), rec.get("context", "")
                picked = json.dumps(rec.get("picked", []), ensure_ascii=False)
                key = json.dumps([items, first, text, picked, context], ensure_ascii=False)
                digest = hashlib.sha256(key.encode("utf-8")).hexdigest()
                cur = self.db.execute("UPDATE records SET count = count + 1 WHERE hash = ?", (digest,))
                if cur.rowcount == 0:
                    self.db.execute(
                        "INSERT INTO records (hash, t, items, first, text, picked, context, style, auto) "
                        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        (digest, rec.get("t", ""), json.dumps(items, ensure_ascii=False), first, text,
                         picked, context, rec.get("style", ""), int(bool(rec.get("auto")))))
                read += 1
        self.db.execute("INSERT OR REPLACE INTO meta VALUES ('offset', ?)", (str(offset),))
        self.db.commit()
        return read

    def clear_raw_log(self) -> None:
        """Empty the engine's log file after importing it (the database keeps the records)."""
        self.import_new()
        if self.log_path.exists():
            self.log_path.write_bytes(b"")
        self.db.execute("INSERT OR REPLACE INTO meta VALUES ('offset', '0')")
        self.db.commit()

    # -- review -------------------------------------------------------------
    def ids(self, condition: str) -> list[int]:
        return [r[0] for r in self.db.execute(f"SELECT id FROM records WHERE {condition} ORDER BY id")]

    def get(self, rec_id: int) -> dict:
        cur = self.db.execute("SELECT * FROM records WHERE id = ?", (rec_id,))
        row = dict(zip([c[0] for c in cur.description], cur.fetchone()))
        row["items"], row["picked"] = json.loads(row["items"]), json.loads(row["picked"])
        return row

    def decide(self, rec_id: int, status: str) -> None:
        self.db.execute("UPDATE records SET status = ? WHERE id = ?", (status, rec_id))
        self.db.commit()

    def reject_containing(self, needle: str) -> int:
        """Reject every record (whatever its status) whose text or context contains `needle`."""
        if not needle:
            return 0
        like = "%" + needle.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_") + "%"
        cur = self.db.execute(
            "UPDATE records SET status = 'rejected' WHERE status <> 'rejected' AND "
            "(text LIKE ? ESCAPE '\\' OR context LIKE ? ESCAPE '\\' OR first LIKE ? ESCAPE '\\')",
            (like, like, like))
        self.db.commit()
        return cur.rowcount

    def delete_rejected(self) -> int:
        """Remove rejected records from the database for good."""
        cur = self.db.execute("DELETE FROM records WHERE status = 'rejected'")
        self.db.commit()
        self.db.execute("VACUUM")
        return cur.rowcount

    def stats(self) -> dict[str, int]:
        out = {PENDING: 0, APPROVED: 0, APPROVED_NO_CONTEXT: 0, REJECTED: 0}
        for status, n in self.db.execute("SELECT status, COUNT(*) FROM records GROUP BY status"):
            out[status] = n
        return out

    # -- export -------------------------------------------------------------
    def export(self, path: Path) -> int:
        """Write the approved records. No timestamps; context only where it was approved."""
        rows = self.db.execute(
            "SELECT items, first, text, picked, context, style, count, status FROM records "
            "WHERE status IN ('approved', 'approved_noctx') ORDER BY id").fetchall()
        tmp = path.with_suffix(path.suffix + ".tmp")
        with open(tmp, "w", encoding="utf-8") as fh:
            for items, first, text, picked, context, style, count, status in rows:
                fh.write(json.dumps({
                    "items": json.loads(items), "first": first, "text": text, "picked": json.loads(picked),
                    "context": context if status == APPROVED else "", "style": style, "count": count,
                }, ensure_ascii=False) + "\n")
        tmp.replace(path)
        return len(rows)


def mark_changes(first: str, text: str) -> str:
    """The committed text with the characters that differ from the engine's choice in 【】."""
    if len(first) != len(text):
        return text
    out, inside = [], False
    for a, b in zip(first, text):
        if (a != b) != inside:
            out.append("【" if a != b else "】")
            inside = a != b
        out.append(b)
    return "".join(out) + ("】" if inside else "")


def run_gui(store: Store) -> None:
    import tkinter as tk
    from tkinter import filedialog, messagebox, ttk

    root = tk.Tk()
    root.title("Bopogram 打字紀錄審核")
    root.geometry("900x560")
    font, big = ("Microsoft JhengHei", 12), ("Microsoft JhengHei", 20)
    state = {"ids": [], "pos": 0}

    top = ttk.Frame(root, padding=8)
    top.pack(fill="x")
    filter_box = ttk.Combobox(top, values=[f[0] for f in FILTERS], state="readonly", width=22, font=font)
    filter_box.current(0)
    filter_box.pack(side="left")
    counter = ttk.Label(top, font=font)
    counter.pack(side="left", padx=12)
    totals = ttk.Label(top, font=font)
    totals.pack(side="right")

    body = ttk.Frame(root, padding=12)
    body.pack(fill="both", expand=True)
    labels = {}
    for key, title, f in (("context", "上文（核准時可選擇不含）", font), ("text", "送出的文字（【】內是你改過的字）", big),
                          ("first", "引擎原本的選字", big), ("items", "輸入的注音", font), ("info", "", font)):
        if title:
            ttk.Label(body, text=title, font=font, foreground="#666").pack(anchor="w", pady=(8, 0))
        labels[key] = tk.Label(body, font=f, anchor="w", justify="left", wraplength=860)
        labels[key].pack(fill="x")

    def refresh(keep_pos: bool = False) -> None:
        state["ids"] = store.ids(FILTERS[filter_box.current()][1])
        state["pos"] = min(state["pos"], max(0, len(state["ids"]) - 1)) if keep_pos else 0
        show()

    def show() -> None:
        s = store.stats()
        totals.config(text=f"待審 {s[PENDING]}　核准 {s[APPROVED] + s[APPROVED_NO_CONTEXT]}　拒絕 {s[REJECTED]}")
        if not state["ids"]:
            counter.config(text="沒有紀錄")
            for lab in labels.values():
                lab.config(text="")
            return
        rec = store.get(state["ids"][state["pos"]])
        counter.config(text=f"第 {state['pos'] + 1} / {len(state['ids'])} 筆")
        labels["context"].config(text=rec["context"] or "（無）", foreground="#888")
        labels["text"].config(text=mark_changes(rec["first"], rec["text"]))
        labels["first"].config(text=rec["first"], foreground="#888" if rec["first"] == rec["text"] else "#b00")
        labels["items"].config(text=" ".join(i[1:] if i.startswith("=") else i for i in rec["items"]))
        status = {PENDING: "待審", APPROVED: "已核准（含上文）", APPROVED_NO_CONTEXT: "已核准（不含上文）",
                  REJECTED: "已拒絕"}[rec["status"]]
        labels["info"].config(text=f"狀態：{status}　出現 {rec['count']} 次　{rec['t']}"
                                   f"{'　自動送出' if rec['auto'] else ''}", foreground="#666")

    def decide(status: str) -> None:
        if not state["ids"]:
            return
        store.decide(state["ids"][state["pos"]], status)
        # In a "pending" view the decided record leaves the list, so the next one moves up.
        if FILTERS[filter_box.current()][1].startswith("status = 'pending'") and status != PENDING:
            refresh(keep_pos=True)
        else:
            move(1)

    def move(delta: int) -> None:
        if state["ids"]:
            state["pos"] = max(0, min(len(state["ids"]) - 1, state["pos"] + delta))
        show()

    def reject_word() -> None:
        needle = needle_var.get().strip()
        if needle and messagebox.askyesno("確認", f"拒絕所有包含「{needle}」的紀錄？\n（包含已核准的）"):
            n = store.reject_containing(needle)
            messagebox.showinfo("完成", f"已拒絕 {n} 筆")
            needle_var.set("")
            refresh(keep_pos=True)

    def export() -> None:
        s = store.stats()
        if s[PENDING] and not messagebox.askyesno("確認", f"還有 {s[PENDING]} 筆待審，它們不會被匯出。繼續？"):
            return
        path = filedialog.asksaveasfilename(initialdir=str(store.userdata), initialfile=EXPORT_NAME,
                                            defaultextension=".jsonl")
        if path:
            messagebox.showinfo("完成", f"已匯出 {store.export(Path(path))} 筆核准的紀錄到\n{path}")

    def clean() -> None:
        if messagebox.askyesno("確認", "刪除引擎的原始紀錄檔內容，並永久刪除已拒絕的紀錄？\n（待審與已核准的紀錄會保留在審核資料庫）"):
            store.clear_raw_log()
            messagebox.showinfo("完成", f"原始紀錄檔已清空，刪除了 {store.delete_rejected()} 筆已拒絕的紀錄")
            refresh()

    buttons = ttk.Frame(root, padding=8)
    buttons.pack(fill="x")
    for text, cmd in (("← 上一筆", lambda: move(-1)), ("下一筆 →", lambda: move(1)),
                      ("核准 (A)", lambda: decide(APPROVED)), ("核准但不含上文 (C)", lambda: decide(APPROVED_NO_CONTEXT)),
                      ("拒絕 (R)", lambda: decide(REJECTED)), ("改回待審 (U)", lambda: decide(PENDING))):
        ttk.Button(buttons, text=text, command=cmd).pack(side="left", padx=3)
    tools = ttk.Frame(root, padding=8)
    tools.pack(fill="x")
    needle_var = tk.StringVar()
    ttk.Label(tools, text="拒絕所有包含此文字的紀錄：", font=font).pack(side="left")
    entry = ttk.Entry(tools, textvariable=needle_var, width=16, font=font)
    entry.pack(side="left")
    ttk.Button(tools, text="全部拒絕", command=reject_word).pack(side="left", padx=3)
    ttk.Button(tools, text="清除原始紀錄與已拒絕的紀錄", command=clean).pack(side="right", padx=3)
    ttk.Button(tools, text="匯出核准的紀錄", command=export).pack(side="right", padx=3)

    def on_key(event) -> None:
        if event.widget is entry:
            return  # typing in the text box must not trigger a decision
        action = {"a": lambda: decide(APPROVED), "c": lambda: decide(APPROVED_NO_CONTEXT),
                  "r": lambda: decide(REJECTED), "u": lambda: decide(PENDING),
                  "left": lambda: move(-1), "right": lambda: move(1)}.get(event.keysym.lower())
        if action:
            action()

    root.bind("<Key>", on_key)
    filter_box.bind("<<ComboboxSelected>>", lambda _e: (root.focus_set(), refresh()))
    store.import_new()
    refresh()
    root.mainloop()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("userdata", type=Path, help="the engine's userdata folder")
    ap.add_argument("--stats", action="store_true", help="import new records and print counts")
    ap.add_argument("--export", type=Path, help="write approved records to this file and exit")
    args = ap.parse_args()
    if not args.userdata.is_dir():
        raise SystemExit(f"{args.userdata} is not a folder")
    store = Store(args.userdata)
    if args.stats or args.export:
        print(f"imported {store.import_new()} new lines; {store.stats()}")
        if args.export:
            print(f"exported {store.export(args.export)} approved records to {args.export}")
        return
    run_gui(store)


if __name__ == "__main__":
    main()
