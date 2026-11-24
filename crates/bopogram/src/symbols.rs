//! Full-width punctuation for symbol keys (`prototype/bopogram/symbols.py`).
//!
//! The front end sends the plain ASCII character of the key (Shift applied);
//! the engine inserts its full-width form. Every ASCII symbol becomes its
//! full-width form (? -> ？), except the keys whose Chinese punctuation is a
//! different character. A symbol in the buffer can be changed to a related
//! one from the candidate list (【 -> 「), see `alternatives`.

const SPECIAL: [(char, char); 6] = [(',', '，'), ('.', '。'), ('\'', '、'), ('[', '【'), (']', '】'), (' ', ' ')];

/// Symbols offered in place of one another, in the order shown.
const GROUPS: [&str; 7] = [
    "【「『〔［｛《〈（",
    "】」』〕］｝》〉）",
    "，、；：",
    "。．…？！",
    "＂“”〝〞",
    "＇‘’",
    "－—～＿",
];

/// Full-width text for one ASCII symbol key, or None if `key` is not a symbol.
pub fn full_width(key: &str) -> Option<String> {
    let mut chars = key.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else { return None };
    if !(' '..='~').contains(&ch) || ch.is_ascii_alphanumeric() {
        return None;
    }
    let wide = SPECIAL
        .iter()
        .find(|(k, _)| *k == ch)
        .map_or_else(|| char::from_u32(ch as u32 + 0xFEE0).unwrap_or(ch), |(_, v)| *v);
    Some(wide.to_string())
}

/// The group of related symbols `ch` belongs to, in a fixed order; empty if none.
pub fn alternatives(ch: &str) -> Vec<String> {
    let mut chars = ch.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return Vec::new() };
    GROUPS
        .iter()
        .find(|g| g.contains(c))
        .map_or_else(Vec::new, |g| g.chars().map(String::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_and_groups() {
        assert_eq!(full_width("?").as_deref(), Some("？"));
        assert_eq!(full_width(",").as_deref(), Some("，"));
        assert_eq!(full_width(" ").as_deref(), Some(" "));
        assert_eq!(full_width("a"), None);
        assert_eq!(full_width("??"), None);
        assert_eq!(alternatives("「")[0], "【");
        assert!(alternatives("x").is_empty());
    }
}
