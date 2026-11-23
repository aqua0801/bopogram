//! ARPA file -> this project's model file (`model.rs`).
//!
//! The values are kept exactly. Orders above unigrams store each probability
//! (and back-off) as an index into a table of that order's distinct values,
//! which needs the ARPA to have at most 256 of them per table: the runtime
//! models were built quantized to 256 values, and their ARPA export carries
//! those values (prototype/docs/lm-arpa.md). An ARPA with more is refused
//! rather than rounded, since rounding would change the scores.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use super::bits::{bits_for, BitArray};
use super::model::{Level, Model, MAX_ORDER, UNK};

/// Bits per word id in the sort keys: vocabularies up to two million words.
const KEY_BITS: u32 = 21;

fn bad(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

struct Gram {
    key: u64,
    prob: f32,
    backoff: Option<f32>,
}

/// Distinct values of one table, in the order first seen.
#[derive(Default)]
struct Table {
    values: Vec<f32>,
    index: HashMap<u32, u8>,
}

impl Table {
    fn add(&mut self, value: f32, what: &str) -> io::Result<()> {
        if !self.index.contains_key(&value.to_bits()) {
            if self.values.len() == 256 {
                return Err(bad(format!(
                    "{what} has more than 256 distinct values: this ARPA is not quantized \
                     (use the export described in prototype/docs/lm-arpa.md)"
                )));
            }
            self.index.insert(value.to_bits(), self.values.len() as u8);
            self.values.push(value);
        }
        Ok(())
    }

    fn of(&self, value: f32) -> u64 {
        self.index[&value.to_bits()] as u64
    }
}

/// What was built. An n-gram "extends" (stays in the state after it) when a
/// longer n-gram starts with it, which is how KenLM's trie models keep their
/// states (checked against them: `bopogram lm-probe`). Many n-grams have a
/// back-off column without any longer n-gram; those counts are reported only
/// for information.
pub struct BuildReport {
    pub counts: Vec<usize>,
    pub extends_without_children: usize,
    pub children_without_backoff: usize,
}

pub fn build(arpa: &Path, out: &Path, progress: &mut dyn FnMut(&str)) -> io::Result<BuildReport> {
    let mut lines = BufReader::with_capacity(1 << 22, File::open(arpa)?).lines();
    // Header: \data\ and the counts.
    let mut counts: Vec<usize> = Vec::new();
    for line in lines.by_ref() {
        let line = line?;
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("ngram ") {
            let (n, count) = rest.split_once('=').ok_or_else(|| bad(format!("bad header line {line:?}")))?;
            let n: usize = n.trim().parse().map_err(|_| bad(format!("bad header line {line:?}")))?;
            if n != counts.len() + 1 {
                return Err(bad("n-gram counts out of order".into()));
            }
            counts.push(count.trim().parse().map_err(|_| bad(format!("bad header line {line:?}")))?);
        } else if line.starts_with("\\1-grams:") {
            break;
        }
    }
    let order = counts.len();
    if !(1..=MAX_ORDER).contains(&order) {
        return Err(bad(format!("order {order} is not supported (1 to {MAX_ORDER})")));
    }

    // Unigrams.
    let size = counts[0];
    let mut vocab = Vec::with_capacity(size);
    let mut ids: HashMap<String, u32> = HashMap::with_capacity(size);
    let mut uni_prob = Vec::with_capacity(size);
    let mut uni_backoff = Vec::with_capacity(size);
    let mut uni_extends = Vec::with_capacity(size);
    let mut section = 1;
    let mut grams: Vec<Vec<Gram>> = (1..order).map(|n| Vec::with_capacity(counts[n])).collect();
    let mut probs: Vec<Table> = (1..order).map(|_| Table::default()).collect();
    let mut backoffs: Vec<Table> = (1..order).map(|_| Table::default()).collect();
    for line in lines {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        if line.starts_with('\\') {
            if line == "\\end\\" {
                break;
            }
            section = line
                .trim_start_matches('\\')
                .split('-')
                .next()
                .and_then(|n| n.parse().ok())
                .ok_or_else(|| bad(format!("bad section {line:?}")))?;
            if section == 2 {
                progress(&format!("{} unigrams", vocab.len()));
            }
            continue;
        }
        let mut fields = line.split('\t');
        let prob: f32 = fields.next().and_then(|v| v.parse().ok()).ok_or_else(|| bad(format!("bad line {line:?}")))?;
        let words = fields.next().ok_or_else(|| bad(format!("bad line {line:?}")))?;
        let backoff: Option<f32> = match fields.next() {
            Some(v) => Some(v.parse().map_err(|_| bad(format!("bad back-off in {line:?}")))?),
            None => None,
        };
        if section == 1 {
            if ids.insert(words.to_owned(), vocab.len() as u32).is_some() {
                return Err(bad(format!("unigram {words:?} twice")));
            }
            vocab.push(words.to_owned());
            uni_prob.push(prob);
            uni_backoff.push(backoff.unwrap_or(0.0));
            uni_extends.push(backoff.is_some());
            continue;
        }
        let mut key = 0u64;
        let mut n = 0;
        for word in words.split(' ') {
            let id = *ids.get(word).ok_or_else(|| bad(format!("{word:?} is not a unigram")))?;
            key = (key << KEY_BITS) | id as u64;
            n += 1;
        }
        if n != section || section > order {
            return Err(bad(format!("{line:?} in the {section}-gram section")));
        }
        if section == order && backoff.is_some() {
            return Err(bad(format!("back-off on a highest-order n-gram: {line:?}")));
        }
        probs[section - 2].add(prob, &format!("{section}-gram probabilities"))?;
        if let Some(b) = backoff {
            backoffs[section - 2].add(b, &format!("{section}-gram back-offs"))?;
        }
        grams[section - 2].push(Gram { key, prob, backoff });
        if grams[section - 2].len().is_multiple_of(5_000_000) {
            progress(&format!("{} {section}-grams", grams[section - 2].len()));
        }
    }
    if vocab.first().map(String::as_str) != Some("<unk>") {
        return Err(bad("the first unigram must be <unk>".into()));
    }
    debug_assert_eq!(UNK, 0);
    if vocab.len() >= 1 << KEY_BITS {
        return Err(bad("vocabulary too large".into()));
    }
    for (n, g) in grams.iter().enumerate() {
        if g.len() != counts[n + 1] {
            return Err(bad(format!("{} {}-grams, the header says {}", g.len(), n + 2, counts[n + 1])));
        }
    }
    for g in grams.iter_mut() {
        g.sort_unstable_by_key(|g| g.key);
        if g.windows(2).any(|w| w[0].key == w[1].key) {
            return Err(bad("an n-gram appears twice".into()));
        }
    }
    progress("sorted");

    // Children: n-grams of order k+1 sorted by key are grouped by their first k words.
    let mut report = BuildReport { counts: counts.clone(), extends_without_children: 0, children_without_backoff: 0 };
    let child_start = |parents: &[u64], children: &[Gram]| -> io::Result<Vec<u64>> {
        // start[i] = first child of parent i; one more entry at the end.
        let mut start = vec![0u64; parents.len() + 1];
        let mut c = 0usize;
        for (i, &p) in parents.iter().enumerate() {
            start[i] = c as u64;
            while c < children.len() && (children[c].key >> KEY_BITS) == p {
                c += 1;
            }
        }
        start[parents.len()] = c as u64;
        if c != children.len() {
            return Err(bad("an n-gram whose first words are not an n-gram".into()));
        }
        Ok(start)
    };
    let uni_keys: Vec<u64> = (0..vocab.len() as u64).collect();
    let uni_child: Vec<u32> = if order > 1 {
        child_start(&uni_keys, &grams[0])?.into_iter().map(|v| v as u32).collect()
    } else {
        vec![0; vocab.len() + 1]
    };
    for i in 0..vocab.len() {
        let has = order > 1 && uni_child[i + 1] > uni_child[i];
        match (uni_extends[i], has) {
            (true, false) => report.extends_without_children += 1,
            (false, true) => report.children_without_backoff += 1,
            _ => {}
        }
        uni_extends[i] = has;
    }

    let word_bits = bits_for(vocab.len() as u64 - 1);
    let mut levels = Vec::new();
    for k in 0..order - 1 {
        let these = &grams[k];
        let middle = k + 2 < order;
        let starts = if middle {
            let keys: Vec<u64> = these.iter().map(|g| g.key).collect();
            child_start(&keys, &grams[k + 1])?
        } else {
            Vec::new()
        };
        let child_bits = if middle { bits_for(*starts.last().unwrap()) } else { 0 };
        let width = Level::record_bits(word_bits, child_bits, middle);
        let mut records = BitArray::new(these.len(), width);
        for (i, g) in these.iter().enumerate() {
            records.set(i, 0, word_bits, g.key & ((1 << KEY_BITS) - 1));
            records.set(i, word_bits, Level::PROB_BITS, probs[k].of(g.prob));
            if middle {
                let mut at = word_bits + Level::PROB_BITS;
                let backoff = g.backoff.map_or(0, |b| backoffs[k].of(b));
                records.set(i, at, Level::BACKOFF_BITS, backoff);
                at += Level::BACKOFF_BITS;
                records.set(i, at, 1, (starts[i + 1] > starts[i]) as u64);
                at += 1;
                records.set(i, at, child_bits, starts[i]);
                let has = starts[i + 1] > starts[i];
                match (g.backoff.is_some(), has) {
                    (true, false) => report.extends_without_children += 1,
                    (false, true) => report.children_without_backoff += 1,
                    _ => {}
                }
            }
        }
        let mut backoff_values = std::mem::take(&mut backoffs[k].values);
        if middle && backoff_values.is_empty() {
            backoff_values.push(0.0);
        }
        levels.push(Level {
            count: these.len(),
            probs: std::mem::take(&mut probs[k].values),
            backoffs: if middle { backoff_values } else { Vec::new() },
            records,
            word_bits,
            child_bits,
            child_end: if middle { *starts.last().unwrap() } else { 0 },
        });
    }
    drop(grams);
    let model = Model::new(vocab, uni_prob, uni_backoff, uni_extends, uni_child, levels);
    model.save(out)?;
    Ok(report)
}
