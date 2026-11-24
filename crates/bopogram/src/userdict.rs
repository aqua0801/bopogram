//! The user's own dictionary: words added by hand, and text excluded from the
//! results (`prototype/bopogram/userdict.py`; rules in
//! `prototype/docs/userdict-engine.md` and `crates/docs/userdict-handoff.md`).
//!
//! Two local files, both in the dict.tsv format (UTF-8, Tab separated, one
//! syllable per character, syllables separated by one space; blank lines and
//! lines starting with "#" are skipped):
//!
//! ```text
//! userdict.tsv   word <Tab> reading <Tab> user
//! excluded.tsv   text <Tab> reading      (only that reading)
//!                text                    (every reading)
//! ```
//!
//! Only the engine writes them. Each change is written at once through a
//! temporary file, and takes effect in memory only once it is on disk. Lines
//! that cannot be read are skipped and reported by line number only, never by
//! content.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::layout::{FINALS, INITIALS, MEDIALS};
use crate::lexicon::MAX_WORD_LEN;

/// The initials that are a syllable on their own.
const SYLLABIC_INITIALS: &str = "ㄓㄔㄕㄖㄗㄘㄙ";
const TONE_MARKS: [char; 3] = ['ˊ', 'ˇ', 'ˋ'];
pub const USER_TAG: &str = "user";

pub type Reading = Vec<String>;

/// A request the dictionary cannot carry out; `reason` is for the front end.
#[derive(Debug)]
pub struct DictError {
    pub reason: &'static str,
    pub message: String,
}

impl DictError {
    pub fn new(reason: &'static str, message: impl Into<String>) -> DictError {
        DictError { reason, message: message.into() }
    }
}

/// Structurally valid bopomofo: [˙] [initial] [medial] [final] [tone mark].
pub fn valid_syllable(syl: &str) -> bool {
    let neutral = syl.starts_with('˙');
    let mut body: Vec<char> = syl.chars().skip(neutral as usize).collect();
    if body.last().is_some_and(|c| TONE_MARKS.contains(c)) {
        if neutral {
            return false;
        }
        body.pop();
    }
    if body.is_empty() {
        return false;
    }
    if body.len() == 1 && INITIALS.contains(body[0]) && !SYLLABIC_INITIALS.contains(body[0]) {
        return false;
    }
    let mut k = 0;
    for group in [INITIALS, MEDIALS, FINALS] {
        if k < body.len() && group.contains(body[k]) {
            k += 1;
        }
    }
    k == body.len()
}

pub fn check_text(text: &str) -> Result<(), DictError> {
    let n = text.chars().count();
    if n == 0 {
        return Err(DictError::new("length_mismatch", "empty text"));
    }
    if n > MAX_WORD_LEN {
        return Err(DictError::new("too_long", format!("at most {MAX_WORD_LEN} characters")));
    }
    Ok(())
}

/// Check the syllables of a reading against `text`.
pub fn parse_reading(text: &str, syls: Reading) -> Result<Reading, DictError> {
    check_text(text)?;
    if syls.is_empty() || !syls.iter().all(|s| valid_syllable(s)) {
        return Err(DictError::new("bad_reading", "the reading has an invalid syllable"));
    }
    if syls.len() != text.chars().count() {
        return Err(DictError::new("length_mismatch", "one syllable per character is required"));
    }
    Ok(syls)
}

pub fn split_reading(reading: &str) -> Reading {
    reading.split_whitespace().map(str::to_owned).collect()
}

/// (line number, fields) of a dict.tsv-style file; a missing file is empty.
fn read_rows(path: Option<&Path>) -> Vec<(usize, Vec<String>)> {
    let Some(path) = path else { return Vec::new() };
    let Ok(bytes) = fs::read(path) else { return Vec::new() };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut rows = Vec::new();
    for (n, line) in text.split_inclusive('\n').enumerate() {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        rows.push((n + 1, line.split('\t').map(str::to_owned).collect()));
    }
    rows
}

fn skip(path: &Path, n: usize) {
    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    eprintln!("{name}: line {n} skipped");
}

/// Text -> readings, in the order first added (the files keep that order).
/// `None` as a reading of an exclusion covers every reading.
pub type Table<R> = Vec<(String, Vec<R>)>;

fn get<'a, R>(table: &'a Table<R>, key: &str) -> Option<&'a Vec<R>> {
    table.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn entry<'a, R>(table: &'a mut Table<R>, key: &str) -> &'a mut Vec<R> {
    let i = match table.iter().position(|(k, _)| k == key) {
        Some(i) => i,
        None => {
            table.push((key.to_owned(), Vec::new()));
            table.len() - 1
        }
    };
    &mut table[i].1
}

fn set<R>(table: &mut Table<R>, key: &str, value: Vec<R>) {
    if value.is_empty() {
        table.retain(|(k, _)| k != key);
    } else {
        *entry(table, key) = value;
    }
}

pub struct UserDict {
    user_path: Option<PathBuf>,
    excluded_path: Option<PathBuf>,
    pub user: Table<Reading>,
    pub excluded: Table<Option<Reading>>,
}

impl UserDict {
    pub fn new(user_path: Option<PathBuf>, excluded_path: Option<PathBuf>) -> UserDict {
        let mut ud = UserDict { user_path, excluded_path, user: Vec::new(), excluded: Vec::new() };
        ud.reload();
        ud
    }

    pub fn reload(&mut self) {
        self.user.clear();
        self.excluded.clear();
        for (n, f) in read_rows(self.user_path.as_deref()) {
            let parsed = if f.len() < 2 { None } else { parse_reading(&f[0], split_reading(&f[1])).ok() };
            let Some(syls) = parsed else {
                skip(self.user_path.as_deref().unwrap(), n);
                continue;
            };
            let have = entry(&mut self.user, &f[0]);
            if !have.contains(&syls) {
                have.push(syls);
            }
        }
        for (n, f) in read_rows(self.excluded_path.as_deref()) {
            let parsed = if f.len() == 1 || f[1].trim().is_empty() {
                check_text(&f[0]).map(|_| None)
            } else {
                parse_reading(&f[0], split_reading(&f[1])).map(Some)
            };
            let Ok(syls) = parsed else {
                skip(self.excluded_path.as_deref().unwrap(), n);
                continue;
            };
            let have = entry(&mut self.excluded, &f[0]);
            if !have.contains(&syls) {
                have.push(syls);
            }
        }
    }

    fn need(path: &Option<PathBuf>) -> Result<&Path, DictError> {
        path.as_deref().ok_or_else(|| DictError::new("disabled", "this feature is turned off in config.json"))
    }

    /// False if the entry was there already.
    pub fn add(&mut self, word: &str, syls: &Reading) -> Result<bool, DictError> {
        Self::need(&self.user_path)?;
        if get(&self.user, word).is_some_and(|have| have.contains(syls)) {
            return Ok(false);
        }
        let mut new = self.user.clone();
        entry(&mut new, word).push(syls.clone());
        self.set_user(new)?;
        Ok(true)
    }

    /// Remove a user word (one reading, or all of them).
    pub fn remove_user(&mut self, word: &str, syls: Option<&Reading>) -> Result<bool, DictError> {
        let Some(have) = get(&self.user, word) else { return Ok(false) };
        if have.is_empty() || syls.is_some_and(|s| !have.contains(s)) {
            return Ok(false);
        }
        Self::need(&self.user_path)?;
        let keep: Vec<Reading> = match syls {
            None => Vec::new(),
            Some(s) => have.iter().filter(|h| *h != s).cloned().collect(),
        };
        let mut new = self.user.clone();
        set(&mut new, word, keep);
        self.set_user(new)?;
        Ok(true)
    }

    pub fn exclude(&mut self, text: &str, syls: Option<&Reading>) -> Result<(), DictError> {
        Self::need(&self.excluded_path)?;
        let have = get(&self.excluded, text).cloned().unwrap_or_default();
        let syls = syls.cloned();
        if have.contains(&None) || have.contains(&syls) {
            return Ok(());
        }
        let mut new = self.excluded.clone();
        // The text without a reading covers the single readings.
        let value = if syls.is_none() { vec![None] } else { [have, vec![syls]].concat() };
        *entry(&mut new, text) = value;
        self.set_excluded(new)
    }

    /// Lift an exclusion: the given reading (and an all-readings entry), or all of them.
    pub fn restore(&mut self, text: &str, syls: Option<&Reading>) -> Result<bool, DictError> {
        let Some(have) = get(&self.excluded, text) else { return Ok(false) };
        if have.is_empty() {
            return Ok(false);
        }
        let keep: Vec<Option<Reading>> = match syls {
            None => Vec::new(),
            Some(s) => have.iter().filter(|h| h.as_ref().is_some_and(|h| h != s)).cloned().collect(),
        };
        if keep.len() == have.len() {
            return Ok(false);
        }
        Self::need(&self.excluded_path)?;
        let mut new = self.excluded.clone();
        set(&mut new, text, keep);
        self.set_excluded(new)?;
        Ok(true)
    }

    /// (word, reading, kind) of every entry: the user's words, then the exclusions.
    pub fn entries(&self) -> Vec<(String, String, &'static str)> {
        let mut out: Vec<(String, String, &'static str)> = Vec::new();
        for (w, ss) in &self.user {
            for s in ss {
                out.push((w.clone(), s.join(" "), "user"));
            }
        }
        for (t, ss) in &self.excluded {
            for s in ss {
                out.push((t.clone(), s.as_ref().map_or(String::new(), |s| s.join(" ")), "excluded"));
            }
        }
        out
    }

    fn write(path: &Path, lines: &[String]) -> Result<(), DictError> {
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            let mut tmp_name = path.as_os_str().to_owned();
            tmp_name.push(".tmp");
            let tmp = PathBuf::from(tmp_name);
            let mut fh = fs::File::create(&tmp)?;
            for line in lines {
                fh.write_all(line.as_bytes())?;
                fh.write_all(b"\n")?;
            }
            fh.sync_all()?;
            drop(fh);
            fs::rename(&tmp, path)
        })();
        result.map_err(|err| {
            let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            DictError::new("write_failed", format!("cannot write {name}: {err}"))
        })
    }

    fn set_user(&mut self, new: Table<Reading>) -> Result<(), DictError> {
        let lines: Vec<String> =
            new.iter().flat_map(|(w, ss)| ss.iter().map(move |s| format!("{w}\t{}\t{USER_TAG}", s.join(" ")))).collect();
        Self::write(Self::need(&self.user_path)?, &lines)?;
        self.user = new;
        Ok(())
    }

    fn set_excluded(&mut self, new: Table<Option<Reading>>) -> Result<(), DictError> {
        let lines: Vec<String> = new
            .iter()
            .flat_map(|(t, ss)| {
                ss.iter().map(move |s| match s {
                    None => t.clone(),
                    Some(s) => format!("{t}\t{}", s.join(" ")),
                })
            })
            .collect();
        Self::write(Self::need(&self.excluded_path)?, &lines)?;
        self.excluded = new;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syllables() {
        for ok in ["ㄅㄛ", "ㄓ", "ㄨㄛˇ", "˙ㄉㄜ", "ㄦˋ", "ㄧ"] {
            assert!(valid_syllable(ok), "{ok}");
        }
        for bad in ["ㄅ", "˙ㄉㄜˇ", "ㄛㄅ", "", "ˇ", "ab"] {
            assert!(!valid_syllable(bad), "{bad}");
        }
    }
}
