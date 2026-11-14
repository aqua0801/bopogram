"""`bopogram serve`: a line-based JSON protocol on stdin/stdout.

No sockets and no network: the front end starts this process and talks to it
through pipes. One request per line, one response per line.

Requests ({"op": ..., "id": optional, ...}):
    key        {"key": "j"}          a layout key (bopomofo symbol or tone)
    backspace
    left / right                     move the candidate cursor
    candidates                       list candidates for the word at the cursor
    select     {"index": 0}          pick a candidate
    commit                           finish the composition
    escape                           discard the composition
    context    {"text": "..."}       text before the cursor (while idle)
    learning   {"enabled": false}    turn learning off (password fields) or on
    state
    quit

Every response has "ok" and the composition state (preedit, pending, cursor,
segments); `commit` adds "commit", `candidates` and `select` add "candidates".
"""

from __future__ import annotations

import json
import sys

from .engine import Engine
from .session import Session


def handle(session: Session, req: dict) -> dict:
    op = req.get("op")
    res: dict = {"ok": True}
    if op == "key":
        res["ok"] = session.key(str(req.get("key", "")))
    elif op == "backspace":
        session.backspace()
    elif op == "left":
        session.move(-1)
    elif op == "right":
        session.move(1)
    elif op == "candidates":
        res["candidates"] = session.candidates()
    elif op == "select":
        res["ok"] = session.select(int(req.get("index", -1)))
    elif op == "commit":
        res["commit"] = session.commit()
    elif op == "escape":
        session.escape()
    elif op == "context":
        session.set_context(str(req.get("text", "")))
    elif op == "learning":
        session.learn = bool(req.get("enabled", True))
    elif op != "state":
        return {"ok": False, "error": f"unknown op {op!r}"}
    res.update(session.state())
    return res


def serve(config: str | None = None, layout: str = "dachen") -> int:
    session = Session(Engine(config), layout)
    out = sys.stdout
    out.write(json.dumps({"ok": True, "ready": True}) + "\n")
    out.flush()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
            if req.get("op") == "quit":
                break
            res = handle(session, req)
        except Exception as err:  # keep serving; report the problem to the front end
            req, res = {}, {"ok": False, "error": f"{type(err).__name__}: {err}"}
        if "id" in req:
            res["id"] = req["id"]
        out.write(json.dumps(res, ensure_ascii=False) + "\n")
        out.flush()
    return 0
