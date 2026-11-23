//! Keyboard layouts: keystrokes -> bopomofo syllables
//! (`prototype/bopogram/layouts/`). The engine only ever sees syllables.

const INITIALS: &str = "ㄅㄆㄇㄈㄉㄊㄋㄌㄍㄎㄏㄐㄑㄒㄓㄔㄕㄖㄗㄘㄙ";
const MEDIALS: &str = "ㄧㄨㄩ";
const FINALS: &str = "ㄚㄛㄜㄝㄞㄟㄠㄡㄢㄣㄤㄥㄦ";
/// The first tone is typed with the space bar and written unmarked.
pub const TONE_FIRST: &str = " ";
pub const TONES: [char; 4] = ['ˊ', 'ˇ', 'ˋ', '˙'];

pub struct Layout {
    pub name: &'static str,
    keys: &'static [(char, &'static str)],
}

#[derive(Debug)]
pub struct LayoutError(pub String);

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The standard (大千 / Dachen) layout.
pub const DACHEN: Layout = Layout {
    name: "dachen",
    keys: &[
        ('1', "ㄅ"), ('q', "ㄆ"), ('a', "ㄇ"), ('z', "ㄈ"),
        ('2', "ㄉ"), ('w', "ㄊ"), ('s', "ㄋ"), ('x', "ㄌ"),
        ('e', "ㄍ"), ('d', "ㄎ"), ('c', "ㄏ"),
        ('r', "ㄐ"), ('f', "ㄑ"), ('v', "ㄒ"),
        ('5', "ㄓ"), ('t', "ㄔ"), ('g', "ㄕ"), ('b', "ㄖ"),
        ('y', "ㄗ"), ('h', "ㄘ"), ('n', "ㄙ"),
        ('u', "ㄧ"), ('j', "ㄨ"), ('m', "ㄩ"),
        ('8', "ㄚ"), ('i', "ㄛ"), ('k', "ㄜ"), (',', "ㄝ"),
        ('9', "ㄞ"), ('o', "ㄟ"), ('l', "ㄠ"), ('.', "ㄡ"),
        ('0', "ㄢ"), ('p', "ㄣ"), (';', "ㄤ"), ('/', "ㄥ"), ('-', "ㄦ"),
        (' ', TONE_FIRST), ('6', "ˊ"), ('3', "ˇ"), ('4', "ˋ"), ('7', "˙"),
    ],
};

pub fn layout(name: &str) -> Option<&'static Layout> {
    (name == "dachen").then_some(&DACHEN)
}

impl Layout {
    /// The symbol (or tone) a key types.
    pub fn symbol(&self, key: char) -> Option<&'static str> {
        let key = key.to_lowercase().next()?;
        self.keys.iter().find(|(k, _)| *k == key).map(|(_, s)| *s)
    }

    /// Keystrokes -> syllables in dictionary notation. Every syllable ends
    /// with a tone key; a later key of the same class replaces the earlier one.
    pub fn to_syllables(&self, keystrokes: &str) -> Result<Vec<String>, LayoutError> {
        let mut out = Vec::new();
        let (mut initial, mut medial, mut fin) = ("", "", "");
        for key in keystrokes.chars() {
            let sym = self
                .symbol(key)
                .ok_or_else(|| LayoutError(format!("key {key:?} is not part of the {} layout", self.name)))?;
            if INITIALS.contains(sym) {
                initial = sym;
            } else if MEDIALS.contains(sym) {
                medial = sym;
            } else if FINALS.contains(sym) {
                fin = sym;
            } else {
                let body = format!("{initial}{medial}{fin}");
                if body.is_empty() {
                    return Err(LayoutError("tone key without a syllable".into()));
                }
                out.push(match sym {
                    TONE_FIRST => body,
                    "˙" => format!("˙{body}"),
                    tone => format!("{body}{tone}"),
                });
                (initial, medial, fin) = ("", "", "");
            }
        }
        if !(initial.is_empty() && medial.is_empty() && fin.is_empty()) {
            return Err(LayoutError("last syllable has no tone key (use space for the first tone)".into()));
        }
        Ok(out)
    }

    /// Inverse of `to_syllables`.
    pub fn to_keys(&self, syllables: &[String]) -> String {
        let key_of = |sym: &str| self.keys.iter().find(|(_, s)| *s == sym).map(|(k, _)| *k);
        let mut out = String::new();
        for syl in syllables {
            let (tone, body) = if let Some(body) = syl.strip_prefix('˙') {
                ("˙".to_owned(), body.to_owned())
            } else if syl.ends_with(TONES) {
                let last = syl.chars().last().unwrap();
                (last.to_string(), syl[..syl.len() - last.len_utf8()].to_owned())
            } else {
                (TONE_FIRST.to_owned(), syl.clone())
            };
            for ch in body.chars() {
                if let Some(k) = key_of(&ch.to_string()) {
                    out.push(k);
                }
            }
            if let Some(k) = key_of(&tone) {
                out.push(k);
            }
        }
        out
    }
}
