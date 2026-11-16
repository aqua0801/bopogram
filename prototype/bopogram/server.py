"""`bopogram serve`: a line-based JSON protocol on stdin/stdout.

No sockets and no network: the front end starts this process and talks to it
through pipes. One request per line, one response per line.

Several input contexts can share one engine process: a request may carry
"sid" (any integer; default 0). Each sid has its own composition buffer and
left context; `close` discards it. The dictionary, the models and the user
model are loaded once and shared.

Requests ({"op": ..., "sid": optional, "id": optional, ...}):
    key        {"key": "j"}          a layout key (bopomofo symbol or tone); Space is
                                     the first tone, or a space when no syllable is
                                     in progress and the buffer is not empty
    symbol     {"key": "<"}          an ASCII symbol key; its full-width form is inserted
    text       {"text": "A"}         characters inserted as they are
    backspace / delete               remove the item before / after the cursor
    left / right / home / end        move the cursor inside the buffer
    candidates                       list candidates for the word at the cursor
    select     {"index": 0}          pick a candidate
    cycle      {"delta": 1}          change the word at the cursor to its next candidate
    commit                           finish the composition
    escape                           discard the syllable in progress, else the buffer
    context    {"text": "..."}       text before the cursor (while idle)
    learning   {"enabled": false}    turn learning AND the typing log off (password
                                     fields) or on
    style      {"style": "formal"}   "casual": lone neutral-tone ㄉㄜ is always 的;
                                     "formal": 的 / 地 / 得 chosen by the model
    state
    close                            forget this sid
    quit                             stop the engine process

Every response has "ok" and the composition state (preedit, pending, cursor,
caret, segments, style); `candidates` and `select` add "candidates".
ANY response may carry "commit": text the front end must insert into the
document before showing the new preedit. `commit` always has it; other
requests have it when the buffer grew too long and its first words were
committed automatically.
"""

from __future__ import annotations

import json
import sys
import time

from .engine import Engine
from .session import Session


def handle(session: Session, req: dict) -> dict:
    op = req.get("op")
    res: dict = {"ok": True}
    if op == "key":
        res["ok"] = session.key(str(req.get("key", "")))
    elif op == "symbol":
        res["ok"] = session.symbol(str(req.get("key", "")))
    elif op == "text":
        res["ok"] = session.text(str(req.get("text", "")))
    elif op == "backspace":
        session.backspace()
    elif op == "delete":
        session.delete()
    elif op == "left":
        session.move(-1)
    elif op == "right":
        session.move(1)
    elif op in ("home", "end"):
        session.move_to(op == "end")
    elif op == "candidates":
        res["candidates"] = session.candidates()
    elif op == "select":
        res["ok"] = session.select(int(req.get("index", -1)))
    elif op == "cycle":
        res["ok"] = session.cycle(int(req.get("delta", 1)))
    elif op == "commit":
        res["commit"] = session.take_flushed() + session.commit()
    elif op == "escape":
        session.escape()
    elif op == "context":
        session.set_context(str(req.get("text", "")))
    elif op == "learning":
        session.learn = bool(req.get("enabled", True))
    elif op == "style":
        res["ok"] = session.set_style(str(req.get("style", "")))
    elif op != "state":
        return {"ok": False, "error": f"unknown op {op!r}"}
    flushed = session.take_flushed()
    if flushed:
        res["commit"] = flushed
    res.update(session.state())
    return res


def serve(config: str | None = None, layout: str = "dachen", trace: str | None = None) -> int:
    """`trace` names a file that receives every request and response with a
    timestamp. It is a debugging aid for front-end work and records everything
    typed, so it is off unless asked for and must not be left on."""
    engine = Engine(config)
    trace_fh = open(trace, "a", encoding="utf-8") if trace else None
    sessions: dict[int, Session] = {}
    out = sys.stdout
    out.write(json.dumps({"ok": True, "ready": True}) + "\n")
    out.flush()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        t0 = time.perf_counter()
        try:
            req = json.loads(line)
            if req.get("op") == "quit":
                break
            sid = int(req.get("sid", 0))
            if req.get("op") == "close":
                sessions.pop(sid, None)
                if engine.user is not None:
                    engine.user.save(force=True)
                res = {"ok": True}
            else:
                session = sessions.get(sid)
                if session is None:
                    session = sessions[sid] = Session(engine, layout)
                res = handle(session, req)
        except Exception as err:  # keep serving; report the problem to the front end
            req, res = {}, {"ok": False, "error": f"{type(err).__name__}: {err}"}
        for field in ("id", "sid"):
            if field in req:
                res[field] = req[field]
        out.write(json.dumps(res, ensure_ascii=False) + "\n")
        out.flush()
        if trace_fh:
            trace_fh.write(f"{time.strftime('%H:%M:%S')} {(time.perf_counter() - t0) * 1000:.1f}ms "
                           f"> {line}\n          < {json.dumps(res, ensure_ascii=False)}\n")
            trace_fh.flush()
    if engine.user is not None:
        engine.user.save(force=True)
    return 0
