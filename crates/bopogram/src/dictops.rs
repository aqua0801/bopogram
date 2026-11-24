//! Protocol operations on the user's dictionary: `readings` and `dict_*`
//! (`prototype/bopogram/dictops.py`; specification in
//! `crates/docs/userdict-handoff.md`).
//!
//! They act on the whole engine, not on one session: a word added in one
//! program can be typed in all of them. The session matters only for the forms
//! that take a span of its buffer (`start` / `length`). The buffer itself is
//! not decoded again here; the session does that on the next key.

use serde_json::{json, Map, Value};

use crate::decoder::is_literal;
use crate::engine::Engine;
use crate::lexicon::{Lexicon, MAX_WORD_LEN};
use crate::session::Session;
use crate::userdict::{check_text, parse_reading, split_reading, DictError, Reading};

pub const OPS: [&str; 7] = ["readings", "dict_add", "dict_remove", "dict_exclude", "dict_restore", "dict_list", "dict_reload"];
const MAX_READINGS: usize = 16;

pub enum OpError {
    /// Refused, with a reason for the front end; the response still carries the state.
    Dict(DictError),
    /// The request cannot be read (as the prototype's exceptions).
    Bad(String),
}

impl From<DictError> for OpError {
    fn from(e: DictError) -> OpError {
        OpError::Dict(e)
    }
}

/// How the request's fields are read (Python's str() and int(), see server.rs).
pub struct Fields<'a> {
    pub req: &'a Map<String, Value>,
    pub text: &'a dyn Fn(&str, &str) -> String,
    pub int: &'a dyn Fn(&str, i64) -> Result<i64, OpError>,
}

/// The syllables of a "reading" field: text split at spaces, or a list of texts.
fn reading_field(value: &Value) -> Result<Reading, OpError> {
    match value {
        Value::String(s) => Ok(split_reading(s)),
        Value::Array(items) => items
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(|| OpError::Bad("AttributeError: bad \"reading\"".into())))
            .collect(),
        _ => Err(OpError::Bad("TypeError: bad \"reading\"".into())),
    }
}

/// Python truthiness of an optional field, for "reading".
fn given(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::Bool(true)) => true,
    }
}

/// The reading given with the request, or `None` (every reading).
fn optional_reading(text: &str, f: &Fields) -> Result<Option<Reading>, OpError> {
    if given(f.req.get("reading")) {
        let syls = reading_field(&f.req["reading"])?;
        return Ok(Some(parse_reading(text, syls)?));
    }
    check_text(text)?;
    Ok(None)
}

fn range(session: &Session, f: &Fields) -> Result<std::ops::Range<usize>, OpError> {
    let start = (f.int)("start", -1)?;
    let length = (f.int)("length", 0)?;
    if start < 0 || length < 1 || (start + length) as usize > session.decoder.syllables.len() {
        return Err(DictError::new("not_found", "the span is not in the composition").into());
    }
    if length as usize > MAX_WORD_LEN {
        return Err(DictError::new("too_long", format!("at most {MAX_WORD_LEN} characters")).into());
    }
    Ok(start as usize..(start + length) as usize)
}

/// Text and typed syllables of a span of the composition buffer.
fn span(session: &Session, f: &Fields) -> Result<(String, Reading), OpError> {
    let r = range(session, f)?;
    let syls: Reading = session.decoder.syllables[r.clone()].to_vec();
    let items = session.decoder.best_items();
    let texts: Vec<&str> = items[r].iter().map(|t| t.0.as_str()).collect();
    if syls.iter().any(|s| is_literal(s)) || texts.iter().any(|t| t.chars().count() != 1) {
        return Err(DictError::new("bad_reading", "the span holds symbols or unconverted syllables").into());
    }
    Ok((texts.concat(), syls))
}

/// Exclude, unless that leaves a syllable with no candidate at all.
fn exclude(engine: &Engine, text: &str, syls: Option<&Reading>) -> Result<(), DictError> {
    if text.chars().count() == 1 {
        let lex = engine.lexicon.borrow();
        let readings: Vec<Reading> = match syls {
            Some(s) => vec![s.clone()],
            None => lex.readings(text).into_iter().map(|(s, _)| s).filter(|s| s.len() == 1).collect(),
        };
        for r in readings {
            let key: Vec<&str> = r.iter().map(String::as_str).collect();
            if !lex.candidates(&key).iter().any(|w| &**w != text) {
                return Err(DictError::new("last_candidate", "this is the only candidate of its reading"));
            }
        }
    }
    engine.userdict.as_ref().expect("checked by the caller").borrow_mut().exclude(text, syls)
}

fn forget(engine: &Engine, text: &str) {
    if let Some(user) = &engine.user {
        user.borrow_mut().forget(text);
    }
}

/// Carry out one request; the fields it adds to the response.
pub fn handle(engine: &Engine, session: &Session, op: &str, f: &Fields) -> Result<Map<String, Value>, OpError> {
    let mut out = Map::new();
    if op == "readings" {
        let text = (f.text)("text", "");
        let list = readings(&engine.lexicon.borrow(), &text)?;
        out.insert(
            "readings".into(),
            list.into_iter().map(|(r, known)| json!({"reading": r, "known": known})).collect::<Vec<_>>().into(),
        );
        return Ok(out);
    }
    let Some(ud) = &engine.userdict else {
        return Err(DictError::new("disabled", "the user dictionary is turned off in config.json").into());
    };
    match op {
        "dict_list" => {
            let entries: Vec<Value> = ud
                .borrow()
                .entries()
                .into_iter()
                .map(|(w, r, kind)| json!({"word": w, "reading": r, "kind": kind}))
                .collect();
            out.insert("entries".into(), entries.into());
        }
        "dict_reload" => {
            ud.borrow_mut().reload();
            engine.refresh_lexicon();
        }
        "dict_add" => {
            let (word, syls) = if f.req.contains_key("word") {
                let word = (f.text)("word", "");
                let syls = match f.req.get("reading") {
                    None => Vec::new(),
                    Some(v) => reading_field(v)?,
                };
                let syls = parse_reading(&word, syls)?;
                (word, syls)
            } else {
                span(session, f)?
            };
            if !ud.borrow_mut().add(&word, &syls)? {
                return Err(DictError::new("duplicate", "the word is in the user dictionary already").into());
            }
            ud.borrow_mut().restore(&word, Some(&syls))?; // adding a word lifts its exclusion
            engine.refresh_lexicon();
        }
        "dict_remove" => {
            let word = (f.text)("word", "");
            let syls = if f.req.contains_key("start") {
                let r = range(session, f)?;
                let syls: Reading = session.decoder.syllables[r].to_vec();
                let key: Vec<&str> = syls.iter().map(String::as_str).collect();
                let found = engine.lexicon.borrow().candidates(&key).iter().any(|w| **w == *word);
                if syls.is_empty() || syls.iter().any(|s| is_literal(s)) || !found {
                    return Err(DictError::new("not_found", "the composition has changed").into());
                }
                Some(syls)
            } else {
                optional_reading(&word, f)?
            };
            let mut removed = None;
            let key: Option<Vec<&str>> = syls.as_ref().map(|s| s.iter().map(String::as_str).collect());
            let in_base = engine.lexicon.borrow().in_base(&word, key.as_deref());
            if in_base {
                // First: it may be refused (last_candidate).
                exclude(engine, &word, syls.as_ref())?;
                removed = Some("excluded");
            }
            if ud.borrow_mut().remove_user(&word, syls.as_ref())? {
                removed = removed.or(Some("user"));
            }
            let Some(removed) = removed else {
                return Err(DictError::new("not_found", "the word is not in the dictionary").into());
            };
            engine.refresh_lexicon();
            forget(engine, &word);
            out.insert("removed".into(), removed.into());
        }
        "dict_exclude" => {
            let text = (f.text)("text", "");
            let syls = optional_reading(&text, f)?;
            exclude(engine, &text, syls.as_ref())?;
            ud.borrow_mut().remove_user(&text, syls.as_ref())?;
            engine.refresh_lexicon();
            forget(engine, &text);
        }
        "dict_restore" => {
            let word = (f.text)("word", "");
            let syls = optional_reading(&word, f)?;
            if !ud.borrow_mut().restore(&word, syls.as_ref())? {
                return Err(DictError::new("not_found", "the text is not excluded").into());
            }
            engine.refresh_lexicon();
        }
        _ => return Err(OpError::Bad(format!("ValueError: unknown op {op:?}"))),
    }
    Ok(out)
}

/// Possible readings of `text`, most likely first, with whether each can be
/// typed (`known`): the dictionary's (or the user's) for a known word,
/// otherwise combinations of each character's readings.
pub fn readings(lex: &Lexicon, text: &str) -> Result<Vec<(String, bool)>, DictError> {
    check_text(text)?;
    let chars: Vec<String> = text.chars().map(String::from).collect();
    let score = |syls: &[String]| -> f64 {
        let mut total = 0.0;
        for (ch, s) in chars.iter().zip(syls) {
            total += lex.reading_logp(ch, s);
        }
        total
    };
    let mut whole: Vec<Reading> = Vec::new();
    for (syls, _) in lex.readings(text) {
        if syls.len() == chars.len() && !whole.contains(&syls) {
            whole.push(syls);
        }
    }
    if !whole.is_empty() {
        let mut ranked: Vec<(f64, Reading)> = whole.into_iter().map(|s| (score(&s), s)).collect();
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        return Ok(ranked
            .into_iter()
            .take(MAX_READINGS)
            .map(|(_, s)| {
                let key: Vec<&str> = s.iter().map(String::as_str).collect();
                (s.join(" "), !lex.blocked(text, &key))
            })
            .collect());
    }
    // Beam over the characters' readings, keeping the MAX_READINGS best partial readings.
    let mut beam: Vec<(f64, Reading)> = vec![(0.0, Vec::new())];
    for ch in &chars {
        let mut options: Vec<String> = Vec::new();
        for (s, _) in lex.readings(ch) {
            if s.len() == 1 && !options.contains(&s[0]) {
                options.push(s[0].clone());
            }
        }
        if options.is_empty() {
            return Ok(Vec::new());
        }
        let mut next: Vec<(f64, Reading)> = Vec::new();
        for (p, syls) in &beam {
            for o in &options {
                let mut s = syls.clone();
                s.push(o.clone());
                next.push((p + lex.reading_logp(ch, o), s));
            }
        }
        next.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        next.truncate(MAX_READINGS);
        beam = next;
    }
    Ok(beam.into_iter().map(|(_, s)| (s.join(" "), false)).collect())
}
