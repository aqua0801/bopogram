"""Protocol operations on the user's dictionary (readings, dict_*).

They act on the whole engine, not on one session: a word added in one
program can be typed in all of them. The session only matters for the forms
that take a span of its composition buffer (`start` / `length`). The buffer
itself is not decoded again here; Session._sync does that on the next key.
See crates/docs/userdict-handoff.md for the specification.
"""

from __future__ import annotations

from .decoder import is_literal
from .lexicon import MAX_WORD_LEN
from .userdict import ANY, DictError, check_text, parse_reading

OPS = ("readings", "dict_add", "dict_remove", "dict_exclude", "dict_restore", "dict_list", "dict_reload")
MAX_READINGS = 16


def handle(engine, session, req: dict) -> dict:
    """Carry out one request; raises DictError with the reason on failure."""
    op = req["op"]
    if op == "readings":
        return {"readings": readings(engine.lexicon, str(req.get("text", "")))}
    ud = getattr(engine, "userdict", None)
    if ud is None:
        raise DictError("disabled", "the user dictionary is turned off in config.json")
    lex = engine.lexicon
    if op == "dict_list":
        return {"entries": ud.entries()}
    if op == "dict_reload":
        ud.reload()
        lex.refresh()
        return {}
    if op == "dict_add":
        if "word" in req:
            word = str(req["word"])
            syls = parse_reading(word, req.get("reading", ""))
        else:
            word, syls = _span(session, req)
        if not ud.add(word, syls):
            raise DictError("duplicate", "the word is in the user dictionary already")
        ud.restore(word, syls)  # adding a word lifts its exclusion
        lex.refresh()
        return {}
    if op == "dict_remove":
        if "start" in req:
            word = str(req.get("word", ""))
            syls = tuple(session.decoder.syllables[_range(session, req)])
            if not syls or any(is_literal(s) for s in syls) or word not in lex.candidates(syls):
                raise DictError("not_found", "the composition has changed")
        else:
            word = str(req.get("word", ""))
            syls = _optional_reading(word, req)
        removed = None
        if lex.in_base(word, syls):  # first: it may be refused (last_candidate)
            _exclude(engine, word, syls)
            removed = "excluded"
        if ud.remove_user(word, syls):
            removed = removed or "user"
        if removed is None:
            raise DictError("not_found", "the word is not in the dictionary")
        lex.refresh()
        _forget(engine, word)
        return {"removed": removed}
    if op == "dict_exclude":
        text = str(req.get("text", ""))
        syls = _optional_reading(text, req)
        _exclude(engine, text, syls)
        ud.remove_user(text, syls)
        lex.refresh()
        _forget(engine, text)
        return {}
    if op == "dict_restore":
        word = str(req.get("word", ""))
        if not ud.restore(word, _optional_reading(word, req)):
            raise DictError("not_found", "the text is not excluded")
        lex.refresh()
        return {}
    raise ValueError(f"unknown op {op!r}")


def _optional_reading(text: str, req: dict):
    if req.get("reading"):
        return parse_reading(text, req["reading"])
    check_text(text)
    return ANY


def _range(session, req: dict) -> slice:
    start, length = int(req.get("start", -1)), int(req.get("length", 0))
    if start < 0 or length < 1 or start + length > len(session.decoder.syllables):
        raise DictError("not_found", "the span is not in the composition")
    if length > MAX_WORD_LEN:
        raise DictError("too_long", f"at most {MAX_WORD_LEN} characters")
    return slice(start, start + length)


def _span(session, req: dict) -> tuple[str, tuple[str, ...]]:
    """Text and typed syllables of a span of the composition buffer."""
    r = _range(session, req)
    syls = tuple(session.decoder.syllables[r])
    texts = [t for t, _, _ in session._items()[r]]
    if any(is_literal(s) for s in syls) or any(len(t) != 1 for t in texts):
        raise DictError("bad_reading", "the span holds symbols or unconverted syllables")
    return "".join(texts), syls


def _exclude(engine, text: str, syls) -> None:
    """Exclude, unless that leaves a syllable with no candidate at all."""
    lex, ud = engine.lexicon, engine.userdict
    if len(text) == 1:
        readings = [syls] if syls is not ANY else [s for s, _ in lex.readings(text) if len(s) == 1]
        for r in readings:
            left = [w for w in lex.candidates(r) if w != text]
            if not left:
                raise DictError("last_candidate", "this is the only candidate of its reading")
    ud.exclude(text, syls)


def _forget(engine, text: str) -> None:
    if getattr(engine, "user", None) is not None:
        engine.user.forget(text)


def readings(lex, text: str) -> list[dict]:
    """Possible readings of `text`, most likely first: the dictionary's (or the
    user's) for a known word, otherwise combinations of each character's readings."""
    check_text(text)

    def known(syls) -> bool:
        return not lex.blocked(text, syls)

    def score(syls) -> float:
        return sum(lex.reading_logp(ch, s) for ch, s in zip(text, syls))

    whole = []
    for syls, _ in lex.readings(text):
        if len(syls) == len(text) and syls not in whole:
            whole.append(syls)
    if whole:
        whole.sort(key=score, reverse=True)
        return [{"reading": " ".join(s), "known": known(s)} for s in whole[:MAX_READINGS]]
    # Beam over the characters' readings, keeping the MAX_READINGS best partial readings.
    beam: list[tuple[float, tuple[str, ...]]] = [(0.0, ())]
    for ch in text:
        options = {s[0] for s, _ in lex.readings(ch) if len(s) == 1}
        if not options:
            return []
        beam = sorted(((p + lex.reading_logp(ch, o), syls + (o,)) for p, syls in beam for o in options),
                      key=lambda x: x[0], reverse=True)[:MAX_READINGS]
    return [{"reading": " ".join(s), "known": False} for _, s in beam]
