//! Requests and responses of the engine protocol: one JSON object per line.
//! The full description is in `prototype/bopogram/server.py`.
//!
//! Shared by the input method (`bopogram-ime`) and the host (`bopogram-host`),
//! so that the two cannot disagree about the pipe or the host's own replies.
//! Whatever engine sits behind the host (the Python prototype today, a Rust
//! engine later), this is the contract.

use serde_json::{json, Value};

/// The named pipe of a user's host, from the user's SID.
pub fn pipe_name(sid: &str) -> String {
    format!(r"\\.\pipe\bopogram-{sid}")
}

/// `%NAME%` replaced by the environment variable NAME, as Windows writes
/// paths in settings files (`bopogram-host.json` is shared by every user and
/// names each user's data as `%LOCALAPPDATA%\Bopogram\...`). An unknown
/// name, or a lone `%`, is left as it is.
pub fn expand_env(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => match std::env::var(&after[..end]) {
                Ok(value) => {
                    out.push_str(&value);
                    rest = &after[end + 1..];
                }
                Err(_) => {
                    out.push('%');
                    rest = after;
                }
            },
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Field present (true) in replies made by the host itself rather than the
/// engine: the engine cannot be reached, so the session is gone.
pub const HOST_ERROR: &str = "host";

/// Field present (true), besides `HOST_ERROR`, while the engine is still
/// loading: the request was not handled, but it will be once the engine is
/// ready, so the client should keep it and send it again.
pub const STARTING: &str = "starting";

/// The host's answer while the engine is loading.
pub fn host_starting(id: Option<&Value>) -> String {
    let mut reply = json!({"ok": false, "error": "engine starting", HOST_ERROR: true, STARTING: true});
    if let Some(id) = id {
        reply["id"] = id.clone();
    }
    reply.to_string()
}

/// A reply of the host's own, answering the request with this `id` (if any).
pub fn host_error(id: Option<&Value>, message: &str) -> String {
    let mut reply = json!({"ok": false, "error": message, HOST_ERROR: true});
    if let Some(id) = id {
        reply["id"] = id.clone();
    }
    reply.to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// A layout key: a bopomofo symbol or a tone.
    Key(char),
    /// An ASCII symbol key (Shift applied); the engine inserts its full-width form.
    Symbol(char),
    /// Characters inserted into the composition as they are.
    Text(String),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Candidates,
    /// Index into the whole candidate list.
    Select(usize),
    /// Change the word at the cursor to its next (1) or previous (-1) candidate.
    Cycle(i32),
    Commit,
    Escape,
    /// Text before the cursor; only meaningful while nothing is being composed.
    Context(String),
    Learning(bool),
    /// "casual" or "formal".
    Style(String),
    State,
    /// Add syllables `start..start + length` of the composition, with the
    /// text shown for them, to the user's dictionary.
    DictAdd { start: usize, length: usize },
    /// Remove `word`, a candidate for syllables `start..start + length`:
    /// from the user's dictionary, or excluded if it is the dictionary's.
    DictRemove { start: usize, length: usize, word: String },
}

impl Op {
    /// The operation's name, without its arguments. A tone key is told apart
    /// from the other layout keys, because it is the one that decodes.
    pub fn name(&self) -> &'static str {
        match self {
            Op::Key(c) if matches!(c, ' ' | '3' | '4' | '6' | '7') => "tone",
            Op::Key(_) => "key",
            Op::Symbol(_) => "symbol",
            Op::Text(_) => "text",
            Op::Backspace => "backspace",
            Op::Delete => "delete",
            Op::Left => "left",
            Op::Right => "right",
            Op::Home => "home",
            Op::End => "end",
            Op::Candidates => "candidates",
            Op::Select(_) => "select",
            Op::Cycle(_) => "cycle",
            Op::Commit => "commit",
            Op::Escape => "escape",
            Op::Context(_) => "context",
            Op::Learning(_) => "learning",
            Op::Style(_) => "style",
            Op::State => "state",
            Op::DictAdd { .. } => "dict_add",
            Op::DictRemove { .. } => "dict_remove",
        }
    }

    pub fn to_json(&self, id: u64) -> String {
        let mut request = match self {
            Op::Key(key) => json!({"op": "key", "key": key.to_string()}),
            Op::Symbol(key) => json!({"op": "symbol", "key": key.to_string()}),
            Op::Text(text) => json!({"op": "text", "text": text}),
            Op::Backspace => json!({"op": "backspace"}),
            Op::Delete => json!({"op": "delete"}),
            Op::Left => json!({"op": "left"}),
            Op::Right => json!({"op": "right"}),
            Op::Home => json!({"op": "home"}),
            Op::End => json!({"op": "end"}),
            Op::Candidates => json!({"op": "candidates"}),
            Op::Select(index) => json!({"op": "select", "index": index}),
            Op::Cycle(delta) => json!({"op": "cycle", "delta": delta}),
            Op::Commit => json!({"op": "commit"}),
            Op::Escape => json!({"op": "escape"}),
            Op::Context(text) => json!({"op": "context", "text": text}),
            Op::Learning(enabled) => json!({"op": "learning", "enabled": enabled}),
            Op::Style(style) => json!({"op": "style", "style": style}),
            Op::State => json!({"op": "state"}),
            Op::DictAdd { start, length } => json!({"op": "dict_add", "start": start, "length": length}),
            Op::DictRemove { start, length, word } => {
                json!({"op": "dict_remove", "start": start, "length": length, "word": word})
            }
        };
        request["id"] = id.into();
        request.to_string()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Response {
    pub ok: bool,
    /// What the composition should show: chosen text plus unfinished symbols.
    pub preedit: String,
    /// Bopomofo symbols of the unfinished syllable (shown at the cursor).
    pub pending: String,
    /// How many characters of the chosen text are before the cursor.
    pub cursor: usize,
    /// Where the insertion point is drawn: a character index into `preedit`.
    pub caret: usize,
    /// The chosen text, word by word, without the unfinished syllable.
    pub segments: Vec<String>,
    /// "casual" or "formal", if the engine reports it.
    pub style: Option<String>,
    pub candidates: Vec<String>,
    /// Syllables each candidate covers: (start, length), as the engine's
    /// `candidates` gives them.
    pub spans: Vec<(usize, usize)>,
    /// Converted syllables in the composition (the unfinished one aside).
    pub syllables: usize,
    /// Text to put into the document before showing the new preedit. Any
    /// response can carry it: the engine commits the front of a long buffer.
    pub commit: Option<String>,
    /// Why a dictionary operation failed (`ok` false): `too_long`,
    /// `bad_reading`, `duplicate`, `not_found`, `last_candidate`, ...
    pub reason: Option<String>,
    /// What `dict_remove` did: "user" (removed) or "excluded".
    pub removed: Option<String>,
}

impl Response {
    /// `None` unless the object carries the composition state. Error replies
    /// (from the host: engine down; from the engine: an internal error) do not.
    pub fn from_value(value: &Value) -> Option<Self> {
        let text = |name: &str| value.get(name)?.as_str().map(str::to_owned);
        let pending = text("pending")?;
        let cursor = value.get("cursor")?.as_u64()? as usize;
        let caret = value
            .get("caret")
            .and_then(Value::as_u64)
            .map_or(cursor + pending.chars().count(), |caret| caret as usize);
        let listed = value.get("candidates").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        let listed: Vec<(String, usize, usize)> = listed
            .iter()
            .filter_map(|c| {
                let number = |name: &str| c.get(name).and_then(Value::as_u64).unwrap_or(0) as usize;
                Some((c.get("word")?.as_str()?.to_owned(), number("start"), number("length")))
            })
            .collect();
        Some(Response {
            ok: value.get("ok")?.as_bool()?,
            preedit: text("preedit")?,
            pending,
            cursor,
            caret,
            segments: value
                .get("segments")?
                .as_array()?
                .iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect(),
            style: text("style"),
            spans: listed.iter().map(|&(_, start, length)| (start, length)).collect(),
            candidates: listed.into_iter().map(|(word, _, _)| word).collect(),
            syllables: value.get("syllables").and_then(Value::as_u64).unwrap_or(0) as usize,
            commit: text("commit"),
            reason: text("reason"),
            removed: text("removed"),
        })
    }
}

/// Why a request has no response. The three cases leave the engine's
/// session in different states, so the caller must treat them differently.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Failure {
    /// Nobody to talk to (host not running, pipe broken, engine restarted).
    /// The session and whatever it was composing are gone.
    Unreachable,
    /// The request was sent and not answered in time. The engine still
    /// handles it, so its session is now ahead of what the caller knows;
    /// text it commits meanwhile comes with the next response.
    TimedOut,
    /// The engine answered that it could not do this (an internal error).
    /// The session is alive and the composition is as it was.
    Rejected,
    /// The engine is loading (or the host was just started). The request
    /// was not handled; send it again later.
    Starting,
}

/// Something that answers requests. A request never blocks for long, and
/// the caller must not retry the same request.
pub trait Engine {
    fn request(&mut self, op: &Op) -> Result<Response, Failure>;

    /// Give up on the session: nothing sent so far may have any effect on
    /// later responses (late commits included).
    fn abandon(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_lines() {
        assert_eq!(Op::Key('j').to_json(7), r#"{"id":7,"key":"j","op":"key"}"#);
        assert_eq!(Op::Select(12).to_json(1), r#"{"id":1,"index":12,"op":"select"}"#);
        assert_eq!(Op::Symbol('<').to_json(1), r#"{"id":1,"key":"<","op":"symbol"}"#);
        assert_eq!(Op::Cycle(1).to_json(1), r#"{"delta":1,"id":1,"op":"cycle"}"#);
        assert_eq!(
            Op::Context("他說\"好\"".into()).to_json(2),
            r#"{"id":2,"op":"context","text":"他說\"好\""}"#
        );
    }

    #[test]
    fn response_with_state() {
        let value: Value = serde_json::from_str(
            r#"{"ok":true,"preedit":"我在ㄐ","pending":"ㄐ","cursor":2,"syllables":2,
                "segments":["我","在"],"candidates":[{"word":"在家","start":1,"length":2}],"id":3}"#,
        )
        .unwrap();
        let response = Response::from_value(&value).unwrap();
        assert_eq!(response.preedit, "我在ㄐ");
        assert_eq!(response.segments, ["我", "在"]);
        assert_eq!(response.candidates, ["在家"]);
        assert_eq!((response.spans.as_slice(), response.syllables), (&[(1, 2)][..], 2));
        assert_eq!(response.commit, None);
        assert_eq!(response.caret, 3);
    }

    #[test]
    fn response_with_caret_style_and_commit() {
        let value: Value = serde_json::from_str(
            r#"{"ok":true,"preedit":"我ㄗ家","pending":"ㄗ","cursor":1,"caret":2,"syllables":2,
                "segments":["我","家"],"style":"casual","commit":"前面"}"#,
        )
        .unwrap();
        let response = Response::from_value(&value).unwrap();
        assert_eq!((response.cursor, response.caret), (1, 2));
        assert_eq!(response.style.as_deref(), Some("casual"));
        assert_eq!(response.commit.as_deref(), Some("前面"));
    }

    #[test]
    fn dictionary_operations() {
        assert_eq!(
            Op::DictAdd { start: 0, length: 2 }.to_json(4),
            r#"{"id":4,"length":2,"op":"dict_add","start":0}"#
        );
        let remove = Op::DictRemove { start: 1, length: 2, word: "視頻".into() };
        assert_eq!(remove.to_json(5), r#"{"id":5,"length":2,"op":"dict_remove","start":1,"word":"視頻"}"#);
        assert_eq!(remove.name(), "dict_remove", "names carry no content");
        // A refusal still carries the state, with the reason.
        let value: Value = serde_json::from_str(
            r#"{"ok":false,"error":"x","reason":"last_candidate","preedit":"視","pending":"","cursor":1,
                "syllables":1,"segments":["視"],"id":6}"#,
        )
        .unwrap();
        let response = Response::from_value(&value).unwrap();
        assert!(!response.ok);
        assert_eq!(response.reason.as_deref(), Some("last_candidate"));
        let removed: Value = serde_json::from_str(
            r#"{"ok":true,"removed":"excluded","preedit":"","pending":"","cursor":0,"syllables":0,"segments":[]}"#,
        )
        .unwrap();
        assert_eq!(Response::from_value(&removed).unwrap().removed.as_deref(), Some("excluded"));
    }

    #[test]
    fn host_errors_are_marked() {
        let reply: Value = serde_json::from_str(&host_error(Some(&json!(7)), "engine timeout")).unwrap();
        assert_eq!(reply["id"], 7);
        assert_eq!(reply[HOST_ERROR], true);
        assert_eq!(Response::from_value(&reply), None);
        assert_eq!(pipe_name("S-1-5-21-1"), r"\\.\pipe\bopogram-S-1-5-21-1");
    }

    #[test]
    fn starting_is_marked() {
        let reply: Value = serde_json::from_str(&host_starting(Some(&json!(3)))).unwrap();
        assert_eq!(reply["id"], 3);
        assert_eq!(reply[STARTING], true);
        assert_eq!(reply[HOST_ERROR], true);
    }

    #[test]
    fn names_carry_no_content() {
        assert_eq!(Op::Key('j').name(), "key");
        assert_eq!(Op::Key('3').name(), "tone");
        assert_eq!(Op::Text("secret".into()).name(), "text");
        assert_eq!(Op::Context("secret".into()).name(), "context");
    }

    #[test]
    fn environment_in_paths() {
        std::env::set_var("BOPOGRAM_TEST_DIR", r"C:\Users\me\AppData\Local");
        assert_eq!(expand_env(r"%BOPOGRAM_TEST_DIR%\Bopogram\data"), r"C:\Users\me\AppData\Local\Bopogram\data");
        assert_eq!(expand_env("100% sure"), "100% sure");
        assert_eq!(expand_env("%NO_SUCH_VAR_HERE%x"), "%NO_SUCH_VAR_HERE%x");
        assert_eq!(expand_env("%%"), "%%");
    }

    #[test]
    fn host_error_is_a_failure() {
        let value: Value =
            serde_json::from_str(r#"{"ok":false,"error":"engine unavailable","id":3}"#).unwrap();
        assert_eq!(Response::from_value(&value), None);
    }
}
