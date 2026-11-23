//! One back-off n-gram model (up to trigrams), in this project's own file
//! format, built from an ARPA file by `build.rs`.
//!
//! Layout: a forward trie. Unigrams are plain arrays (probability, back-off,
//! and where their bigrams start). Each higher order is one array of packed
//! records sorted by the whole n-gram, holding the last word, the probability
//! as an 8-bit index into that order's table of values, and for orders below
//! the highest the back-off (also an index), whether the n-gram can be
//! extended (has children), and where its children start. The children of an n-gram are
//! the records from its start to the start of the next n-gram's.
//!
//! Scores are log10, computed in f32 the way back-off models are evaluated:
//! the longest n-gram found gives the probability, and the back-off weights
//! of the longer contexts it did not match are added, shortest first.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use super::bits::BitArray;

pub const MAX_ORDER: usize = 3;
/// Context words a state can hold.
pub const MAX_CONTEXT: usize = MAX_ORDER - 1;
const MAGIC: &[u8; 8] = b"BGLM0001";

/// What a model remembers of the text so far: the last few words, most
/// recent first, as many as can still be extended by a longer n-gram.
/// Two states are the same when their words are; the back-off weights follow
/// from the words and are kept only to save looking them up again.
#[derive(Clone, Copy, Debug)]
pub struct State {
    pub len: u8,
    pub words: [u32; MAX_CONTEXT],
    pub backoff: [f32; MAX_CONTEXT],
}

impl State {
    pub const EMPTY: State = State { len: 0, words: [0; MAX_CONTEXT], backoff: [0.0; MAX_CONTEXT] };

    fn key(&self) -> &[u32] {
        &self.words[..self.len as usize]
    }
}

impl PartialEq for State {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for State {}

impl std::hash::Hash for State {
    fn hash<H: std::hash::Hasher>(&self, hasher: &mut H) {
        self.key().hash(hasher);
    }
}

/// One order above unigrams.
pub(crate) struct Level {
    pub count: usize,
    pub probs: Vec<f32>,
    /// Empty for the highest order.
    pub backoffs: Vec<f32>,
    pub records: BitArray,
    pub word_bits: u32,
    pub child_bits: u32,
    /// Where the children of the record after the last one would start.
    pub child_end: u64,
}

impl Level {
    pub const PROB_BITS: u32 = 8;
    pub const BACKOFF_BITS: u32 = 8;

    fn middle(&self) -> bool {
        !self.backoffs.is_empty() || self.child_bits > 0
    }

    #[inline]
    fn word(&self, i: usize) -> u32 {
        self.records.get(i, 0, self.word_bits) as u32
    }

    #[inline]
    fn prob(&self, i: usize) -> f32 {
        self.probs[self.records.get(i, self.word_bits, Self::PROB_BITS) as usize]
    }

    #[inline]
    fn backoff(&self, i: usize) -> f32 {
        self.backoffs[self.records.get(i, self.word_bits + Self::PROB_BITS, Self::BACKOFF_BITS) as usize]
    }

    #[inline]
    fn extends(&self, i: usize) -> bool {
        self.records.get(i, self.word_bits + Self::PROB_BITS + Self::BACKOFF_BITS, 1) == 1
    }

    #[inline]
    fn child_start(&self, i: usize) -> usize {
        if i == self.count {
            return self.child_end as usize;
        }
        self.records.get(i, self.word_bits + Self::PROB_BITS + Self::BACKOFF_BITS + 1, self.child_bits) as usize
    }

    /// The record for `word` among records `range`, which are sorted by word.
    fn find(&self, range: (usize, usize), word: u32) -> Option<usize> {
        let (mut low, mut high) = range;
        while low < high {
            let mid = low + (high - low) / 2;
            let at = self.word(mid);
            if at == word {
                return Some(mid);
            }
            if at < word {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        None
    }

    pub(crate) fn record_bits(word_bits: u32, child_bits: u32, middle: bool) -> u32 {
        word_bits + Self::PROB_BITS + if middle { Self::BACKOFF_BITS + 1 + child_bits } else { 0 }
    }
}

pub struct Model {
    pub vocab: Vec<String>,
    pub(crate) index: HashMap<String, u32>,
    pub(crate) uni_prob: Vec<f32>,
    pub(crate) uni_backoff: Vec<f32>,
    pub(crate) uni_extends: Vec<bool>,
    /// Where each unigram's bigrams start; one more entry than the vocabulary.
    pub(crate) uni_child: Vec<u32>,
    pub(crate) levels: Vec<Level>,
}

/// The word id of `<unk>`: always the first.
pub const UNK: u32 = 0;

impl Model {
    pub(crate) fn new(
        vocab: Vec<String>,
        uni_prob: Vec<f32>,
        uni_backoff: Vec<f32>,
        uni_extends: Vec<bool>,
        uni_child: Vec<u32>,
        levels: Vec<Level>,
    ) -> Model {
        let index = vocab.iter().enumerate().map(|(i, w)| (w.clone(), i as u32)).collect();
        Model { vocab, index, uni_prob, uni_backoff, uni_extends, uni_child, levels }
    }

    pub fn order(&self) -> usize {
        self.levels.len() + 1
    }

    pub fn id(&self, word: &str) -> Option<u32> {
        self.index.get(word).copied()
    }

    /// The state at the start of a sentence (after `<s>`).
    pub fn begin_sentence(&self, s: u32) -> State {
        let mut state = State::EMPTY;
        state.len = 1;
        state.words[0] = s;
        state.backoff[0] = self.uni_backoff[s as usize];
        state
    }

    /// log10 P(word | state) and the state after it.
    pub fn score(&self, state: &State, word: u32) -> (f32, State) {
        let mut out = State::EMPTY;
        let w = word as usize;
        let context = state.len as usize;
        let mut prob = self.uni_prob[w];
        // Length of the longest n-gram found that ends in `word`.
        let mut matched = 1usize;
        if word != UNK {
            if self.uni_extends[w] {
                out.len = 1;
                out.words[0] = word;
                out.backoff[0] = self.uni_backoff[w];
            }
            for n in 2..=self.order().min(context + 1) {
                // The n-gram is state.words[n-2] .. state.words[0], word. Its context
                // exists (a state only holds n-grams that do); `word` is looked up
                // among the context's continuations.
                let range = if n == 2 {
                    let c = state.words[0] as usize;
                    (self.uni_child[c] as usize, self.uni_child[c + 1] as usize)
                } else {
                    let Some(record) = self.locate(&state.words[..n - 1]) else { break };
                    let below = &self.levels[n - 3];
                    (below.child_start(record), below.child_start(record + 1))
                };
                let level = &self.levels[n - 2];
                let Some(found) = level.find(range, word) else { break };
                prob = level.prob(found);
                matched = n;
                if n <= MAX_CONTEXT && level.middle() && level.extends(found) {
                    out.len = n as u8;
                    out.words[1..n].copy_from_slice(&state.words[..n - 1]);
                    out.backoff[n - 1] = level.backoff(found);
                }
            }
        }
        // Back off over the context the n-gram found does not cover, shortest first.
        for i in (matched - 1)..context {
            prob += state.backoff[i];
        }
        (prob, out)
    }

    /// The record of the n-gram `words` (most recent first) in its level.
    fn locate(&self, words: &[u32]) -> Option<usize> {
        let n = words.len();
        if n < 2 || n > self.levels.len() + 1 {
            return None;
        }
        let first = words[n - 1] as usize;
        let mut range = (self.uni_child[first] as usize, self.uni_child[first + 1] as usize);
        let mut record = None;
        for k in (0..n - 1).rev() {
            let level = &self.levels[n - 2 - k];
            let found = level.find(range, words[k])?;
            record = Some(found);
            if k > 0 {
                range = (level.child_start(found), level.child_start(found + 1));
            }
        }
        record
    }

    // -- file ----------------------------------------------------------------

    pub fn load(path: &Path) -> io::Result<Model> {
        let mut file = BufReader::with_capacity(1 << 20, File::open(path)?);
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{}: not a Bopogram model", path.display())));
        }
        let order = read_u32(&mut file)? as usize;
        if !(1..=MAX_ORDER).contains(&order) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unsupported order {order}")));
        }
        let size = read_u32(&mut file)? as usize;
        let mut vocab = Vec::with_capacity(size);
        for _ in 0..size {
            let len = read_u32(&mut file)? as usize;
            let mut bytes = vec![0u8; len];
            file.read_exact(&mut bytes)?;
            vocab.push(String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?);
        }
        let uni_prob = read_f32s(&mut file, size)?;
        let uni_backoff = read_f32s(&mut file, size)?;
        let uni_extends = read_bytes(&mut file, size)?.into_iter().map(|b| b != 0).collect();
        let uni_child = if order > 1 { read_u32s(&mut file, size + 1)? } else { vec![0; size + 1] };
        let mut levels = Vec::new();
        for k in 2..=order {
            let count = read_u64(&mut file)? as usize;
            let n = read_u32(&mut file)? as usize;
            let probs = read_f32s(&mut file, n)?;
            let backoffs = if k < order {
                let n = read_u32(&mut file)? as usize;
                read_f32s(&mut file, n)?
            } else {
                Vec::new()
            };
            let word_bits = read_u32(&mut file)?;
            let child_bits = read_u32(&mut file)?;
            let child_end = read_u64(&mut file)?;
            let width = Level::record_bits(word_bits, child_bits, k < order);
            let words = read_u64(&mut file)? as usize;
            let data = read_u64s(&mut file, words)?;
            levels.push(Level {
                count,
                probs,
                backoffs,
                records: BitArray { words: data, width },
                word_bits,
                child_bits,
                child_end,
            });
        }
        Ok(Model::new(vocab, uni_prob, uni_backoff, uni_extends, uni_child, levels))
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut file = BufWriter::with_capacity(1 << 20, File::create(path)?);
        file.write_all(MAGIC)?;
        write_u32(&mut file, self.order() as u32)?;
        write_u32(&mut file, self.vocab.len() as u32)?;
        for word in &self.vocab {
            write_u32(&mut file, word.len() as u32)?;
            file.write_all(word.as_bytes())?;
        }
        for v in &self.uni_prob {
            file.write_all(&v.to_le_bytes())?;
        }
        for v in &self.uni_backoff {
            file.write_all(&v.to_le_bytes())?;
        }
        let extends: Vec<u8> = self.uni_extends.iter().map(|&e| e as u8).collect();
        file.write_all(&extends)?;
        if self.order() > 1 {
            for v in &self.uni_child {
                write_u32(&mut file, *v)?;
            }
        }
        for (i, level) in self.levels.iter().enumerate() {
            write_u64(&mut file, level.count as u64)?;
            write_u32(&mut file, level.probs.len() as u32)?;
            for v in &level.probs {
                file.write_all(&v.to_le_bytes())?;
            }
            if i + 2 < self.order() {
                write_u32(&mut file, level.backoffs.len() as u32)?;
                for v in &level.backoffs {
                    file.write_all(&v.to_le_bytes())?;
                }
            }
            write_u32(&mut file, level.word_bits)?;
            write_u32(&mut file, level.child_bits)?;
            write_u64(&mut file, level.child_end)?;
            write_u64(&mut file, level.records.words.len() as u64)?;
            for v in &level.records.words {
                write_u64(&mut file, *v)?;
            }
        }
        file.flush()
    }

    /// Bytes of memory the model's tables take.
    pub fn memory(&self) -> usize {
        self.uni_prob.len() * 9
            + self.uni_child.len() * 4
            + self.vocab.iter().map(|w| w.len() + 24).sum::<usize>()
            + self.levels.iter().map(|l| l.records.words.len() * 8).sum::<usize>()
    }
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn read_bytes(r: &mut impl Read, n: usize) -> io::Result<Vec<u8>> {
    let mut v = vec![0u8; n];
    r.read_exact(&mut v)?;
    Ok(v)
}

fn read_f32s(r: &mut impl Read, n: usize) -> io::Result<Vec<f32>> {
    Ok(read_bytes(r, n * 4)?.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect())
}

fn read_u32s(r: &mut impl Read, n: usize) -> io::Result<Vec<u32>> {
    Ok(read_bytes(r, n * 4)?.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect())
}

/// Read in pieces: the largest arrays are hundreds of megabytes, and reading
/// them through one byte buffer would need twice that for a moment.
fn read_u64s(r: &mut impl Read, n: usize) -> io::Result<Vec<u64>> {
    let mut out = Vec::with_capacity(n);
    let mut buf = vec![0u8; 1 << 20];
    let mut left = n;
    while left > 0 {
        let take = left.min(buf.len() / 8);
        r.read_exact(&mut buf[..take * 8])?;
        out.extend(buf[..take * 8].chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())));
        left -= take;
    }
    Ok(out)
}

fn write_u32(w: &mut impl Write, v: u32) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn write_u64(w: &mut impl Write, v: u64) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lm::build::build;

    /// A small trigram model: scores follow the back-off rules, and states keep
    /// only words that a longer n-gram can extend.
    #[test]
    fn scores_and_states() {
        let arpa = [
            r"\data\",
            "ngram 1=5",
            "ngram 2=3",
            "ngram 3=1",
            "",
            r"\1-grams:",
            "-7\t<unk>",
            "-99\t<s>\t-0.5",
            "-1\t</s>",
            "-1.5\ta\t-0.25",
            "-2\tb\t-0.125",
            "",
            r"\2-grams:",
            "-0.75\t<s> a\t-0.0625",
            "-0.5\ta b",
            "-1.25\tb a",
            "",
            r"\3-grams:",
            "-0.25\t<s> a b",
            "",
            r"\end\",
            "",
        ]
        .join("\n");
        let dir = std::env::temp_dir().join(format!("bopogram-lm-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, out) = (dir.join("t.arpa"), dir.join("t.bglm"));
        std::fs::write(&src, arpa).unwrap();
        build(&src, &out, &mut |_| {}).unwrap();
        let m = Model::load(&out).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        let id = |w| m.id(w).unwrap();
        let start = m.begin_sentence(id("<s>"));
        // <s> a: the bigram.
        let (p, after_a) = m.score(&start, id("a"));
        assert_eq!(p, -0.75);
        assert_eq!(after_a.len, 2, "<s> a has a trigram after it");
        // <s> a b: the trigram; (a b) has no continuation, so the state keeps b only.
        let (p, after_b) = m.score(&after_a, id("b"));
        assert_eq!(p, -0.25);
        assert_eq!(&after_b.words[..after_b.len as usize], &[id("b")]);
        // b </s>: no bigram; unigram plus the back-off of b.
        let (p, end) = m.score(&after_b, id("</s>"));
        assert_eq!(p, -1.0 + -0.125);
        assert_eq!(end.len, 0);
        // <s> a then a: no trigram, no bigram (a a): unigram + bo(a) + bo(<s> a).
        let (p, _) = m.score(&after_a, id("a"));
        assert_eq!(p, -1.5f32 + -0.25 + -0.0625);
        // An unknown word: <unk> and every back-off, and an empty state.
        let (p, unk) = m.score(&after_a, UNK);
        assert_eq!(p, -7.0f32 + -0.25 + -0.0625);
        assert_eq!(unk.len, 0);
    }
}
