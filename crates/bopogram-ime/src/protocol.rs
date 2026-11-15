//! Requests and responses of the engine protocol: one JSON object per line.
//! The full description is in `prototype/bopogram/server.py`.

use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// A layout key: a bopomofo symbol or a tone.
    Key(char),
    Backspace,
    Left,
    Right,
    Candidates,
    /// Index into the whole candidate list.
    Select(usize),
    Commit,
    Escape,
    /// Text before the cursor; only meaningful while nothing is being composed.
    Context(String),
    Learning(bool),
    State,
}

impl Op {
    pub fn to_json(&self, id: u64) -> String {
        let mut request = match self {
            Op::Key(key) => json!({"op": "key", "key": key.to_string()}),
            Op::Backspace => json!({"op": "backspace"}),
            Op::Left => json!({"op": "left"}),
            Op::Right => json!({"op": "right"}),
            Op::Candidates => json!({"op": "candidates"}),
            Op::Select(index) => json!({"op": "select", "index": index}),
            Op::Commit => json!({"op": "commit"}),
            Op::Escape => json!({"op": "escape"}),
            Op::Context(text) => json!({"op": "context", "text": text}),
            Op::Learning(enabled) => json!({"op": "learning", "enabled": enabled}),
            Op::State => json!({"op": "state"}),
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
    /// Bopomofo symbols of the unfinished syllable (the tail of `preedit`).
    pub pending: String,
    /// Candidate cursor: after how many characters.
    pub cursor: usize,
    /// The chosen text, word by word, without the unfinished syllable.
    pub segments: Vec<String>,
    pub candidates: Vec<String>,
    pub commit: Option<String>,
}

impl Response {
    /// `None` unless the object carries the composition state. Error replies
    /// produced by the host (engine down, timeout) do not, and count as failures.
    pub fn from_value(value: &Value) -> Option<Self> {
        let text = |name: &str| value.get(name)?.as_str().map(str::to_owned);
        Some(Response {
            ok: value.get("ok")?.as_bool()?,
            preedit: text("preedit")?,
            pending: text("pending")?,
            cursor: value.get("cursor")?.as_u64()? as usize,
            segments: value
                .get("segments")?
                .as_array()?
                .iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect(),
            candidates: value
                .get("candidates")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|c| c.get("word")?.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            commit: text("commit"),
        })
    }
}

/// Something that answers requests. `None` means the engine could not be
/// reached in time; the caller must not block or retry on the same key.
pub trait Engine {
    fn request(&mut self, op: &Op) -> Option<Response>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_lines() {
        assert_eq!(Op::Key('j').to_json(7), r#"{"id":7,"key":"j","op":"key"}"#);
        assert_eq!(Op::Select(12).to_json(1), r#"{"id":1,"index":12,"op":"select"}"#);
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
        assert_eq!(response.commit, None);
    }

    #[test]
    fn host_error_is_a_failure() {
        let value: Value =
            serde_json::from_str(r#"{"ok":false,"error":"engine unavailable","id":3}"#).unwrap();
        assert_eq!(Response::from_value(&value), None);
    }
}
