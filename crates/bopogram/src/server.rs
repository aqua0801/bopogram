//! `bopogram serve`: a line-based JSON protocol on stdin/stdout
//! (`prototype/bopogram/server.py`, `prototype/docs/frontend-handoff.md`).
//!
//! No sockets and no network: the front end starts this process and talks to
//! it through pipes. One request per line, one response per line.
//!
//! Several input contexts can share one engine process: a request may carry
//! "sid" (any integer; default 0). Each sid has its own composition buffer and
//! left context; `close` discards it. The dictionary and the models are
//! loaded once and shared.
//!
//! Every response has "ok" and the composition state (preedit, pending,
//! cursor, caret, syllables, segments, style); `candidates` and `select` add
//! "candidates". ANY response may carry "commit": text the front end must
//! insert into the document before showing the new preedit.
//!
//! Requests are read as the prototype reads them, including its leniency:
//! "index", "delta" and "sid" may be numbers or numeric strings, other fields
//! are taken as text, "enabled" by truthiness. A request that cannot be read
//! gets `{"ok": false, "error": ...}` without its "id".

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use std::rc::Rc;

use crate::dictops::{self, Fields, OpError, OPS};
use crate::engine::Engine;
use crate::layout::Layout;
use crate::session::Session;

/// Python's `str()` of a JSON value, as far as requests need it.
fn py_str(value: Option<&Value>, default: &str) -> String {
    match value {
        None => default.to_owned(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(true)) => "True".into(),
        Some(Value::Bool(false)) => "False".into(),
        Some(Value::Null) => "None".into(),
        Some(other) => other.to_string(),
    }
}

/// Python's `int()` of a JSON value; `None` where Python raises.
fn py_int(value: Option<&Value>, default: i64) -> Option<i64> {
    match value {
        None => Some(default),
        Some(Value::Bool(b)) => Some(*b as i64),
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)),
        Some(Value::String(s)) => {
            let s = s.trim();
            let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
            let ok = !digits.is_empty()
                && !digits.starts_with('_')
                && !digits.ends_with('_')
                && !digits.contains("__")
                && digits.chars().all(|c| c.is_ascii_digit() || c == '_');
            if ok {
                s.replace('_', "").parse().ok()
            } else {
                None
            }
        }
        Some(_) => None,
    }
}

/// Python's truthiness of a JSON value.
fn py_truthy(value: Option<&Value>, default: bool) -> bool {
    match value {
        None => default,
        Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// A request that cannot be read, reported like the prototype's exceptions.
struct BadRequest(String);

fn handle(session: &mut Session, req: &Map<String, Value>) -> Result<Map<String, Value>, BadRequest> {
    let op = req.get("op").and_then(Value::as_str).unwrap_or("");
    let mut res = Map::new();
    let mut ok = true;
    let int = |name: &str, default: i64| {
        py_int(req.get(name), default).ok_or_else(|| BadRequest(format!("ValueError: bad {name:?}")))
    };
    match op {
        "key" => ok = session.key(&py_str(req.get("key"), "")),
        "symbol" => ok = session.symbol(&py_str(req.get("key"), "")),
        "text" => ok = session.text(&py_str(req.get("text"), "")),
        "backspace" => session.backspace(),
        "delete" => session.delete(),
        "left" => session.move_by(-1),
        "right" => session.move_by(1),
        "home" | "end" => session.move_to(op == "end"),
        "candidates" => {
            res.insert("candidates".into(), candidates_json(session));
        }
        "select" => ok = session.select(int("index", -1)?),
        "cycle" => ok = session.cycle(int("delta", 1)?),
        "commit" => {
            let text = session.take_flushed() + &session.commit();
            res.insert("commit".into(), Value::String(text));
        }
        "escape" => session.escape(),
        "context" => session.set_context(&py_str(req.get("text"), "")),
        "learning" => session.learn = py_truthy(req.get("enabled"), true),
        "style" => ok = session.set_style(&py_str(req.get("style"), "")),
        "state" => {}
        _ if OPS.contains(&op) => {
            let text = |name: &str, default: &str| py_str(req.get(name), default);
            let int = |name: &str, default: i64| {
                py_int(req.get(name), default).ok_or_else(|| OpError::Bad(format!("ValueError: bad {name:?}")))
            };
            let fields = Fields { req, text: &text, int: &int };
            let engine = Rc::clone(&session.engine);
            match dictops::handle(&engine, session, op, &fields) {
                Ok(out) => res.extend(out),
                Err(OpError::Dict(err)) => {
                    ok = false;
                    res.insert("error".into(), Value::String(err.message));
                    res.insert("reason".into(), Value::String(err.reason.into()));
                }
                Err(OpError::Bad(message)) => return Err(BadRequest(message)),
            }
        }
        _ => {
            let shown = req.get("op").map_or("None".to_owned(), |v| v.to_string());
            let mut out = Map::new();
            out.insert("ok".into(), Value::Bool(false));
            out.insert("error".into(), Value::String(format!("unknown op {shown}")));
            return Ok(out);
        }
    }
    res.insert("ok".into(), Value::Bool(ok));
    let flushed = session.take_flushed();
    if !flushed.is_empty() {
        res.insert("commit".into(), Value::String(flushed));
    }
    let state = session.state();
    res.insert("preedit".into(), state.preedit.into());
    res.insert("pending".into(), state.pending.into());
    res.insert("cursor".into(), state.cursor.into());
    res.insert("caret".into(), state.caret.into());
    res.insert("syllables".into(), state.syllables.into());
    res.insert("segments".into(), state.segments.into());
    res.insert("style".into(), state.style.name().into());
    Ok(res)
}

fn candidates_json(session: &Session) -> Value {
    Value::Array(
        session
            .candidates()
            .into_iter()
            .map(|c| json!({"word": c.word, "start": c.start, "length": c.length}))
            .collect(),
    )
}

fn error(message: String) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("ok".into(), Value::Bool(false));
    out.insert("error".into(), Value::String(message));
    out
}

/// Wall-clock time of day (UTC) for the trace.
fn clock() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("{:02}:{:02}:{:02}Z", secs / 3600 % 24, secs / 60 % 60, secs % 60)
}

/// Serve requests until `quit` or the end of the input.
///
/// `trace` receives every request and response with the time taken. It is a
/// debugging aid for front-end work and records everything typed, so it is
/// off unless asked for and must not be left on.
///
/// The user model is saved however serving ends: also when the front end goes
/// away while a response is being written (the pipe then fails).
pub fn serve(
    engine: Rc<Engine>,
    layout: &'static Layout,
    input: impl BufRead,
    out: impl Write,
    trace: Option<Box<dyn Write>>,
) -> io::Result<()> {
    let result = serve_lines(&engine, layout, input, out, trace);
    engine.save_user();
    result
}

fn serve_lines(
    engine: &Rc<Engine>,
    layout: &'static Layout,
    input: impl BufRead,
    mut out: impl Write,
    mut trace: Option<Box<dyn Write>>,
) -> io::Result<()> {
    let mut sessions: HashMap<i64, Session> = HashMap::new();
    writeln!(out, "{}", json!({"ok": true, "ready": true}))?;
    out.flush()?;
    for line in input.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let started = Instant::now();
        let parsed: Result<Map<String, Value>, BadRequest> = match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(req)) => Ok(req),
            Ok(_) => Err(BadRequest("AttributeError: the request is not an object".into())),
            Err(e) => Err(BadRequest(format!("JSONDecodeError: {e}"))),
        };
        if let Ok(req) = &parsed {
            if req.get("op").and_then(Value::as_str) == Some("quit") {
                break;
            }
        }
        // A bug must not take the engine down with every session in it: the
        // request gets an error, as the prototype answers its exceptions.
        let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match parsed {
            Err(BadRequest(message)) => error(message),
            Ok(req) => {
                let outcome = match py_int(req.get("sid"), 0) {
                    None => Err(BadRequest("ValueError: bad \"sid\"".into())),
                    Some(sid) if req.get("op").and_then(Value::as_str) == Some("close") => {
                        sessions.remove(&sid);
                        engine.save_user();
                        let mut res = Map::new();
                        res.insert("ok".into(), Value::Bool(true));
                        Ok(res)
                    }
                    Some(sid) => {
                        let session = sessions.entry(sid).or_insert_with(|| Session::new(Rc::clone(engine), layout));
                        handle(session, &req)
                    }
                };
                match outcome {
                    Ok(mut res) => {
                        for field in ["id", "sid"] {
                            if let Some(v) = req.get(field) {
                                res.insert(field.into(), v.clone());
                            }
                        }
                        res
                    }
                    Err(BadRequest(message)) => error(message),
                }
            }
        }));
        let res = answer.unwrap_or_else(|_| error("InternalError: the engine failed on this request".into()));
        let text = Value::Object(res).to_string();
        writeln!(out, "{text}")?;
        out.flush()?;
        if let Some(t) = trace.as_mut() {
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            write!(t, "{} {ms:.1}ms > {line}\n          < {text}\n", clock())?;
            t.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_conversions() {
        assert_eq!(py_int(Some(&json!("12")), 0), Some(12));
        assert_eq!(py_int(Some(&json!(" -3 ")), 0), Some(-3));
        assert_eq!(py_int(Some(&json!("1_0")), 0), Some(10));
        assert_eq!(py_int(Some(&json!(2.9)), 0), Some(2));
        assert_eq!(py_int(Some(&json!(true)), 0), Some(1));
        assert_eq!(py_int(Some(&json!("x")), 0), None);
        assert_eq!(py_int(Some(&Value::Null), 0), None);
        assert_eq!(py_int(None, 7), Some(7));
        assert_eq!(py_str(Some(&json!(1)), ""), "1");
        assert!(!py_truthy(Some(&json!(0)), true));
        assert!(py_truthy(Some(&json!("no")), false));
    }
}
