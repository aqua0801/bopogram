//! Personalisation: a small user n-gram learned from committed text
//! (`prototype/bopogram/user.py`).
//!
//! Only word unigram and bigram counts are stored, in a local JSON file. The
//! model adds a bonus to the decoder score; it never changes the base model or
//! any character frequency. Nothing here touches the network, and nothing is
//! learned while learning is off (password fields).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

/// The file is written at most this often, unless forced.
const SAVE_INTERVAL: Duration = Duration::from_secs(30);

pub struct UserModel {
    path: Option<PathBuf>,
    weight: f64,
    /// Share of the word count in the bonus.
    unigram_weight: f64,
    /// A word picked by hand counts this many times.
    correction_step: u64,
    /// A word taken as the engine chose it. 0: learning accepted text only
    /// reinforces what the models already prefer (see user.py for the numbers).
    accepted_step: u64,
    unigrams: HashMap<String, u64>,
    bigrams: HashMap<String, u64>,
    saved_at: Option<Instant>,
    dirty: bool,
}

fn counts(value: Option<&Value>) -> HashMap<String, u64> {
    let Some(Value::Object(map)) = value else { return HashMap::new() };
    map.iter().filter_map(|(k, v)| Some((k.clone(), v.as_u64()?))).collect()
}

impl UserModel {
    pub fn load(path: Option<PathBuf>) -> io::Result<UserModel> {
        let mut model = UserModel {
            path,
            weight: 2.0,
            unigram_weight: 0.3,
            correction_step: 3,
            accepted_step: 0,
            unigrams: HashMap::new(),
            bigrams: HashMap::new(),
            saved_at: None,
            dirty: false,
        };
        if let Some(path) = model.path.as_ref().filter(|p| p.exists()) {
            let text = fs::read_to_string(path)?;
            let data: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display())))?;
            model.unigrams = counts(data.get("unigrams"));
            model.bigrams = counts(data.get("bigrams"));
        }
        Ok(model)
    }

    /// Record a committed word sequence; `picked` are the indices of the words
    /// the user chose by hand. A pair counts as a correction when either word was picked.
    pub fn learn(&mut self, words: &[String], picked: &HashSet<usize>) {
        let mut prev: Option<&String> = None;
        for (k, w) in words.iter().enumerate() {
            let step = if picked.contains(&k) { self.correction_step } else { self.accepted_step };
            if step > 0 {
                *self.unigrams.entry(w.clone()).or_default() += step;
                self.dirty = true;
            }
            if let Some(p) = prev {
                let pair = picked.contains(&k) || (k > 0 && picked.contains(&(k - 1)));
                let pstep = if pair { self.correction_step } else { self.accepted_step };
                if pstep > 0 {
                    *self.bigrams.entry(format!("{p}\t{w}")).or_default() += pstep;
                    self.dirty = true;
                }
            }
            prev = Some(w);
        }
    }

    /// Added log score for choosing `word` after `prev`.
    pub fn bonus(&self, prev: Option<&str>, word: &str) -> f64 {
        let b = match prev {
            Some(p) if !p.is_empty() => self.bigrams.get(&format!("{p}\t{word}")).copied().unwrap_or(0),
            _ => 0,
        };
        let u = self.unigrams.get(word).copied().unwrap_or(0);
        if b == 0 && u == 0 {
            return 0.0;
        }
        self.weight * ((b as f64).ln_1p() + self.unigram_weight * (u as f64).ln_1p())
    }

    /// Drop every word and word pair whose text contains `text` (when the user
    /// removes or excludes it); the file is saved. Returns how many entries went.
    pub fn forget(&mut self, text: &str) -> usize {
        let before = self.unigrams.len() + self.bigrams.len();
        self.unigrams.retain(|w, _| !w.contains(text));
        self.bigrams.retain(|k, _| !k.replace('\t', "").contains(text));
        let gone = before - self.unigrams.len() - self.bigrams.len();
        if gone > 0 {
            self.dirty = true;
            self.save(true);
        }
        gone
    }

    /// Write the file, at most once every SAVE_INTERVAL unless `force`:
    /// rewriting it on every commit would make each commit slower as it grows.
    /// A failure leaves the counts in memory to be written next time.
    pub fn save(&mut self, force: bool) {
        let Some(path) = self.path.clone() else { return };
        if !self.dirty {
            return;
        }
        if !force && self.saved_at.is_some_and(|t| t.elapsed() < SAVE_INTERVAL) {
            return;
        }
        self.saved_at = Some(Instant::now());
        let to_map = |m: &HashMap<String, u64>| -> Map<String, Value> {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            keys.into_iter().map(|k| (k.clone(), Value::from(m[k]))).collect()
        };
        let mut data = Map::new();
        data.insert("unigrams".into(), Value::Object(to_map(&self.unigrams)));
        data.insert("bigrams".into(), Value::Object(to_map(&self.bigrams)));
        let result = (|| -> io::Result<()> {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("tmp");
            fs::write(&tmp, Value::Object(data).to_string())?;
            fs::rename(&tmp, &path)
        })();
        match result {
            Ok(()) => self.dirty = false,
            Err(err) => eprintln!("user model: cannot save: {err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_corrections_only() {
        let mut m = UserModel::load(None).unwrap();
        let words: Vec<String> = ["他", "是", "我", "的", "朋友"].iter().map(|s| s.to_string()).collect();
        m.learn(&words, &[2].into_iter().collect());
        assert_eq!(m.unigrams.len(), 1);
        assert_eq!(m.bigrams.len(), 2);
        assert!((m.bonus(Some("是"), "我") - 2.0 * (4f64.ln() + 0.3 * 4f64.ln())).abs() < 1e-12);
        assert_eq!(m.bonus(None, "朋友"), 0.0);
        assert_eq!(m.forget("我"), 3);
    }
}
