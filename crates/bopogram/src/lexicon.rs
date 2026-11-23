//! Pronunciation dictionary: bopomofo syllable sequences -> candidate words
//! (`prototype/bopogram/lexicon.py`).
//!
//! `dict.tsv`: word <Tab> syllables separated by spaces [<Tab> source tag].

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::rc::Rc;

pub const MAX_WORD_LEN: usize = 8;
pub const READING_FLOOR: f64 = -8.0;

/// Spellings accepted as input that stand for another reading.
pub fn reading_alias(syllable: &str) -> &str {
    match syllable {
        "˙ㄇㄛ" => "˙ㄇㄜ",
        other => other,
    }
}

pub type Word = Rc<str>;

pub struct Lexicon {
    /// Syllable -> id, for the keys below.
    syllables: HashMap<String, u32>,
    by_reading: HashMap<Box<[u32]>, Vec<Word>>,
    readings: HashMap<String, Vec<(Vec<String>, String)>>,
    reading_logp: HashMap<(String, String), f64>,
    /// Goes up with every change of the user's dictionary, so decoders know
    /// their results are stale.
    pub version: u64,
    pub user_bonus: f64,
    user_by_reading: HashMap<Box<[u32]>, Vec<Word>>,
    user_entries: HashSet<(String, Vec<String>)>,
    user_readings: HashMap<String, Vec<Vec<String>>>,
    /// Excluded text -> readings (`None` = every reading).
    pub excluded: HashMap<String, Vec<Option<Vec<String>>>>,
    pub excluded_lengths: Vec<usize>,
    pub excluded_prefixes: HashSet<String>,
}

impl Lexicon {
    pub fn load(path: &Path) -> io::Result<Lexicon> {
        let text = fs::read_to_string(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        let mut lexicon = Lexicon {
            syllables: HashMap::new(),
            by_reading: HashMap::new(),
            readings: HashMap::new(),
            reading_logp: HashMap::new(),
            version: 0,
            user_bonus: 0.0,
            user_by_reading: HashMap::new(),
            user_entries: HashSet::new(),
            user_readings: HashMap::new(),
            excluded: HashMap::new(),
            excluded_lengths: Vec::new(),
            excluded_prefixes: HashSet::new(),
        };
        let mut words: HashMap<String, Word> = HashMap::new();
        for line in text.split('\n') {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 2 {
                continue;
            }
            let word = parts[0];
            let syls: Vec<String> = parts[1].split_whitespace().map(str::to_owned).collect();
            let tag = parts.get(2).copied().unwrap_or("").to_owned();
            let key = lexicon.intern(&syls);
            let shared = words.entry(word.to_owned()).or_insert_with(|| Rc::from(word)).clone();
            let list = lexicon.by_reading.entry(key).or_default();
            if !list.iter().any(|w| **w == *word) {
                list.push(shared);
            }
            lexicon.readings.entry(word.to_owned()).or_default().push((syls, tag));
        }
        Ok(lexicon)
    }

    fn intern(&mut self, syllables: &[String]) -> Box<[u32]> {
        syllables
            .iter()
            .map(|s| {
                let next = self.syllables.len() as u32;
                *self.syllables.entry(s.clone()).or_insert(next)
            })
            .collect()
    }

    /// The key of `syllables`, if every one of them is in the dictionary.
    fn key(&self, syllables: &[&str]) -> Option<Box<[u32]>> {
        syllables.iter().map(|s| self.syllables.get(*s).copied()).collect()
    }

    /// Table of P(syllable | character) (`training/neural/reading_counts.py`):
    /// character <Tab> syllable <Tab> probability. For a character in the table,
    /// a reading scores weight * ln P, never below weight * READING_FLOOR.
    pub fn load_reading_prior(&mut self, path: &Path, weight: f64) -> io::Result<()> {
        let text = fs::read_to_string(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        let mut probs: HashMap<&str, HashMap<&str, f64>> = HashMap::new();
        for line in text.split('\n') {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() == 3 {
                let p: f64 = parts[2]
                    .trim()
                    .parse()
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("bad line {line:?}")))?;
                probs.entry(parts[0]).or_default().insert(parts[1], p);
            }
        }
        for (ch, seen) in &probs {
            let Some(readings) = self.readings.get(*ch) else { continue };
            for (syls, _) in readings {
                let syl = reading_alias(&syls[0]);
                let p = seen.get(syl).copied().unwrap_or(0.0);
                let logp = if p > 0.0 { p.ln() } else { READING_FLOOR };
                self.reading_logp.insert(((*ch).to_owned(), syl.to_owned()), weight * READING_FLOOR.max(logp));
            }
        }
        Ok(())
    }

    /// ln P(syllable | char) for a single character; 0.0 when unknown.
    pub fn reading_logp(&self, ch: &str, syllable: &str) -> f64 {
        // Looked up without allocating: the table is small and keyed by owned strings.
        self.reading_logp.get(&(ch.to_owned(), reading_alias(syllable).to_owned())).copied().unwrap_or(0.0)
    }

    pub fn contains(&self, word: &str) -> bool {
        self.readings.contains_key(word) || self.user_readings.contains_key(word)
    }

    pub fn is_user_active(&self) -> bool {
        !self.user_entries.is_empty()
    }

    pub fn is_user(&self, word: &str, syllables: &[&str]) -> bool {
        !self.user_entries.is_empty()
            && self.user_entries.contains(&(word.to_owned(), syllables.iter().map(|s| (*s).to_owned()).collect()))
    }

    /// Does `text` (read as `syllables`, one per character) contain excluded text?
    pub fn blocked(&self, text: &str, syllables: &[&str]) -> bool {
        if self.excluded.is_empty() {
            return false;
        }
        let chars: Vec<char> = text.chars().collect();
        for &n in &self.excluded_lengths {
            if n > chars.len() {
                continue;
            }
            for k in 0..=chars.len() - n {
                let piece: String = chars[k..k + n].iter().collect();
                if let Some(readings) = self.excluded.get(&piece) {
                    let typed: Vec<String> = syllables[k..(k + n).min(syllables.len())].iter().map(|s| (*s).to_owned()).collect();
                    if readings.iter().any(|r| r.as_ref().is_none_or(|r| *r == typed)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Is the word (with this reading) in the dictionary itself, not the user's?
    pub fn in_base(&self, word: &str, syllables: Option<&[&str]>) -> bool {
        self.readings
            .get(word)
            .is_some_and(|r| r.iter().any(|(s, _)| syllables.is_none_or(|want| s.iter().map(String::as_str).eq(want.iter().copied()))))
    }

    /// Words pronounced exactly as `syllables` (one syllable per character):
    /// the user's words first, then the dictionary's, without excluded text.
    pub fn candidates(&self, syllables: &[&str]) -> Vec<Word> {
        let key = self.key(syllables);
        let base: &[Word] = key.as_ref().and_then(|k| self.by_reading.get(k)).map_or(&[], Vec::as_slice);
        if self.user_by_reading.is_empty() && self.excluded.is_empty() {
            return base.to_vec();
        }
        let user: &[Word] = key.as_ref().and_then(|k| self.user_by_reading.get(k)).map_or(&[], Vec::as_slice);
        let words: Vec<Word> = if user.is_empty() {
            base.to_vec()
        } else {
            user.iter().cloned().chain(base.iter().filter(|w| !user.contains(w)).cloned()).collect()
        };
        words.into_iter().filter(|w| !self.blocked(w, syllables)).collect()
    }

    /// All (syllables, source tag) pairs of a word, the user's readings included.
    pub fn readings(&self, word: &str) -> Vec<(Vec<String>, String)> {
        let base = self.readings.get(word).cloned().unwrap_or_default();
        let Some(user) = self.user_readings.get(word) else { return base };
        let mut out = base.clone();
        for s in user {
            if !base.iter().any(|(b, _)| b == s) {
                out.push((s.clone(), "user".to_owned()));
            }
        }
        out
    }
}
