//! Lattice decoding: bopomofo syllables -> most probable word sequence
//! (`prototype/bopogram/decoder.py`; keep the two in step).
//!
//! The lattice has one node per syllable boundary. An edge from i to j is a
//! dictionary word pronounced as syllables[i..j]. Hypotheses ending at the
//! same node with the same language-model state are merged (Viterbi); each
//! node keeps at most `beam` hypotheses. Ties keep the order in which the
//! hypotheses were made, as the prototype's stable sort does.
//!
//! The buffer may also hold literal characters, stored as "=" + character;
//! a literal occupies one position, is never converted, and no word spans it.

use std::collections::HashMap;
use std::rc::Rc;

use crate::lexicon::{Lexicon, Word, MAX_WORD_LEN};
use crate::lm::{MixtureLM, StateRef, Token, UNKNOWN_CHAR};

const PARTICLE_SYLLABLE: &str = "˙ㄉㄜ";
const PARTICLE_CASUAL: &str = "的";
const PARTICLE_FORMAL: [&str; 2] = ["地", "得"];
pub const LITERAL: char = '=';

/// Tone sandhi: 一 and 不 typed with their spoken tone are valid only before
/// a syllable whose tone causes the change; until it arrives the choice
/// carries SANDHI_PENALTY.
const SANDHI_PENALTY: f64 = -4.0;

fn sandhi_tones(word: &str, syllable: &str) -> Option<&'static [u8]> {
    match (word, syllable) {
        ("一", "ㄧˊ") => Some(&[4]),
        ("一", "ㄧˋ") => Some(&[1, 2, 3]),
        ("不", "ㄅㄨˊ") => Some(&[4]),
        _ => None,
    }
}

fn sandhi_also(word: &str, syllable: &str, next: &str) -> bool {
    (word, syllable) == ("一", "ㄧˊ") && next == "˙ㄍㄜ"
}

fn is_sandhi_syllable(syllable: &str) -> bool {
    matches!(syllable, "ㄧˊ" | "ㄧˋ" | "ㄅㄨˊ")
}

/// 1-4, or 5 for the neutral tone.
pub fn tone_of(syllable: &str) -> u8 {
    if syllable.starts_with('˙') {
        return 5;
    }
    match syllable.chars().last() {
        Some('ˊ') => 2,
        Some('ˇ') => 3,
        Some('ˋ') => 4,
        _ => 1,
    }
}

/// Buffer item for a character that is passed through unchanged.
pub fn literal(ch: &str) -> String {
    format!("{LITERAL}{ch}")
}

pub fn is_literal(item: &str) -> bool {
    item.starts_with(LITERAL)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Casual,
    Formal,
}

impl Style {
    pub fn parse(name: &str) -> Option<Style> {
        match name {
            "casual" => Some(Style::Casual),
            "formal" => Some(Style::Formal),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Style::Casual => "casual",
            Style::Formal => "formal",
        }
    }
}

pub struct Hyp {
    pub score: f64,
    pub state: StateRef,
    /// Last word (`None` for the start hypothesis).
    pub word: Option<Word>,
    /// Node where `word` starts.
    pub start: usize,
    pub prev: Option<Rc<Hyp>>,
    /// The end of the path's text that could still grow into excluded text;
    /// always empty while nothing is excluded.
    pub tail: String,
}

/// Hypotheses merged by slot, in the order the slots were first made.
#[derive(PartialEq, Eq, Hash)]
struct Slot {
    state: StateRef,
    tail: String,
    /// Set when hypotheses with different histories are kept apart.
    origin: Option<usize>,
}

#[derive(Default)]
struct Merged {
    order: Vec<Rc<Hyp>>,
    index: HashMap<Slot, usize>,
}

impl Merged {
    fn offer(&mut self, slot: Slot, make: impl FnOnce() -> Hyp, score: f64) {
        match self.index.get(&slot) {
            Some(&i) => {
                if score > self.order[i].score {
                    self.order[i] = Rc::new(make());
                }
            }
            None => {
                self.index.insert(slot, self.order.len());
                self.order.push(Rc::new(make()));
            }
        }
    }

    /// Keep the first hypothesis for a slot (the prototype's `setdefault`).
    fn first(&mut self, slot: Slot, make: impl FnOnce() -> Hyp) {
        if !self.index.contains_key(&slot) {
            self.index.insert(slot, self.order.len());
            self.order.push(Rc::new(make()));
        }
    }
}

pub type ExtraScore = Rc<dyn Fn(Option<&str>, &str) -> f64>;

/// Sort best first; equal scores keep their order.
fn sort_best_first<T>(items: &mut [T], score: impl Fn(&T) -> f64) {
    items.sort_by(|a, b| score(b).partial_cmp(&score(a)).unwrap_or(std::cmp::Ordering::Equal));
}

pub struct Decoder {
    pub lexicon: Rc<std::cell::RefCell<Lexicon>>,
    pub lm: Rc<MixtureLM>,
    pub beam: usize,
    pub max_candidates: usize,
    pub extra_score: Option<ExtraScore>,
    pub word_penalty: f64,
    pub eos_weight: f64,
    pub word_bias: Rc<HashMap<String, f64>>,
    pub style: Style,
    /// False keeps hypotheses with different histories apart instead of
    /// merging those with the same model state (used with the re-ranker).
    pub merge: bool,
    /// Result chosen by an outside re-ranker among `nbest`; cleared whenever
    /// the buffer changes.
    pub choice: Option<Vec<Word>>,
    cand_cache: HashMap<Vec<String>, Vec<Word>>,
    version: u64,
    /// Spans the user fixed by picking a candidate: (start, end) -> word.
    pub pinned: Vec<((usize, usize), Word)>,
    pub syllables: Vec<String>,
    pub nodes: Vec<Vec<Rc<Hyp>>>,
    unk_floor: Option<f64>,
    unknown_char: Token,
}

impl Decoder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        lexicon: Rc<std::cell::RefCell<Lexicon>>,
        lm: Rc<MixtureLM>,
        beam: usize,
        max_candidates: usize,
        extra_score: Option<ExtraScore>,
        word_penalty: f64,
        eos_weight: f64,
        style: Style,
        word_bias: Rc<HashMap<String, f64>>,
    ) -> Decoder {
        let version = lexicon.borrow().version;
        let unknown_char = lm.token(UNKNOWN_CHAR);
        let start = lm.begin();
        let mut decoder = Decoder {
            lexicon,
            lm,
            beam,
            max_candidates,
            extra_score,
            word_penalty,
            eos_weight,
            word_bias,
            style,
            merge: true,
            choice: None,
            cand_cache: HashMap::new(),
            version,
            pinned: Vec::new(),
            syllables: Vec::new(),
            nodes: Vec::new(),
            unk_floor: None,
            unknown_char,
        };
        decoder.reset(Some(start));
        decoder
    }

    fn start_hyp(state: StateRef) -> Rc<Hyp> {
        Rc::new(Hyp { score: 0.0, state, word: None, start: 0, prev: None, tail: String::new() })
    }

    // -- composition buffer ---------------------------------------------------

    pub fn reset(&mut self, start_state: Option<StateRef>) {
        self.syllables.clear();
        self.pinned.clear();
        let state = start_state.unwrap_or_else(|| self.lm.begin());
        self.nodes = vec![vec![Self::start_hyp(state)]];
        self.choice = None;
    }

    pub fn start_state(&self) -> StateRef {
        Rc::clone(&self.nodes[0][0].state)
    }

    fn pinned_get(&self, span: (usize, usize)) -> Option<&Word> {
        self.pinned.iter().find(|(s, _)| *s == span).map(|(_, w)| w)
    }

    fn candidates(&mut self, key: &[String]) -> Vec<Word> {
        let lexicon = self.lexicon.borrow();
        if self.version != lexicon.version {
            self.cand_cache.clear();
            self.version = lexicon.version;
        }
        if let Some(hit) = self.cand_cache.get(key) {
            return hit.clone();
        }
        if key.iter().any(|item| is_literal(item)) {
            return if key.len() == 1 { vec![Rc::from(&key[0][LITERAL.len_utf8()..])] } else { Vec::new() };
        }
        let syls: Vec<&str> = key.iter().map(String::as_str).collect();
        let all = lexicon.candidates(&syls);
        // Words the model has never seen cannot win on their own; leave them out
        // unless nothing else fits. The user's own words always stay.
        let lm = &self.lm;
        let floor = *self.unk_floor.get_or_insert_with(|| lm.unigram("<unk>") + 1e-6);
        let user: Vec<Word> = all.iter().filter(|w| lexicon.is_user(w, &syls)).cloned().collect();
        let known: Vec<Word> =
            all.iter().filter(|w| !user.contains(w) && lm.unigram(w) > floor).cloned().collect();
        let mut cands: Vec<Word> = user.iter().chain(known.iter()).cloned().collect();
        if cands.is_empty() {
            cands = all;
        }
        if cands.len() > self.max_candidates {
            let mut rest: Vec<(Word, f64)> = cands
                .iter()
                .filter(|w| !user.contains(w))
                .map(|w| {
                    let mut rank = lm.unigram(w);
                    if key.len() == 1 {
                        rank += lexicon.reading_logp(w, &key[0]);
                    }
                    (Rc::clone(w), rank)
                })
                .collect();
            sort_best_first(&mut rest, |r| r.1);
            let room = self.max_candidates.saturating_sub(user.len());
            cands = user.iter().cloned().chain(rest.into_iter().take(room).map(|r| r.0)).collect();
        }
        drop(lexicon);
        self.cand_cache.insert(key.to_vec(), cands.clone());
        cands
    }

    /// True when the dictionary changed since the lattice was built.
    pub fn stale(&self) -> bool {
        self.version != self.lexicon.borrow().version
    }

    /// Decode the buffer again if the dictionary changed; true if it did.
    pub fn refresh(&mut self) -> bool {
        if !self.stale() {
            return false;
        }
        let pinned = self.pinned.clone();
        self.repin(pinned, None);
        true
    }

    fn pinned_at(&self, pos: usize) -> bool {
        self.pinned.iter().any(|((a, b), _)| *a <= pos && pos < *b)
    }

    /// The tail after appending `word` (starting at item i) to a path ending
    /// in `tail`, or `None` if that forms excluded text somewhere. Text the
    /// user picked by hand is allowed.
    fn extend(&self, lexicon: &Lexicon, tail: &str, i: usize, word: &str) -> Option<String> {
        let text: Vec<char> = tail.chars().chain(word.chars()).collect();
        let tail_len = tail.chars().count();
        let base = i as isize - tail_len as isize;
        for end in tail_len + 1..=text.len() {
            for &n in &lexicon.excluded_lengths {
                if n > end {
                    break;
                }
                let piece: String = text[end - n..end].iter().collect();
                let Some(readings) = lexicon.excluded.get(&piece) else { continue };
                if readings.is_empty() {
                    continue;
                }
                let a = (base + (end - n) as isize) as usize;
                let typed: Vec<String> = self.syllables[a..(a + n).min(self.syllables.len())].to_vec();
                if !readings.iter().any(|r| r.as_ref().is_none_or(|r| *r == typed)) {
                    continue;
                }
                if !(a..a + n).all(|p| self.pinned_at(p)) {
                    return None;
                }
            }
        }
        for k in (1..=text.len().min(MAX_WORD_LEN - 1)).rev() {
            let end: String = text[text.len() - k..].iter().collect();
            if lexicon.excluded_prefixes.contains(&end) {
                return Some(end);
            }
        }
        Some(String::new())
    }

    /// An edge may not cut through a pinned span. It may cover one if it has
    /// the picked text there.
    fn allowed(&self, i: usize, j: usize, word: &str) -> bool {
        let chars: Vec<char> = word.chars().collect();
        for ((a, b), fixed) in &self.pinned {
            let (a, b) = (*a, *b);
            if j <= a || i >= b {
                continue;
            }
            if i > a || j < b {
                return false;
            }
            let piece: String = chars.get(a - i..b - i).map(|c| c.iter().collect()).unwrap_or_default();
            if piece != **fixed {
                return false;
            }
        }
        true
    }

    pub fn set_style(&mut self, style: Style) {
        if style != self.style {
            self.style = style;
            let pinned = self.pinned.clone();
            self.repin(pinned, None);
        }
    }

    /// Replace the pinned spans and decode the current buffer again.
    pub fn repin(&mut self, pinned: Vec<((usize, usize), Word)>, start_state: Option<StateRef>) {
        let syllables = std::mem::take(&mut self.syllables);
        let state = start_state.unwrap_or_else(|| self.start_state());
        self.pinned = pinned;
        self.nodes = vec![vec![Self::start_hyp(state)]];
        self.choice = None;
        for s in syllables {
            self.push(s);
        }
    }

    /// Append one syllable and build the lattice node after it.
    pub fn push(&mut self, syllable: String) {
        self.syllables.push(syllable);
        self.choice = None;
        let j = self.syllables.len();
        let mut merged = Merged::default();
        let lexicon_rc = Rc::clone(&self.lexicon);
        let excluding = !lexicon_rc.borrow().excluded.is_empty();
        for i in j.saturating_sub(MAX_WORD_LEN)..j {
            if self.nodes[i].is_empty() {
                continue;
            }
            let mut hyps: Vec<Rc<Hyp>> = self.nodes[i].clone();
            // A 一 / 不 typed with its sandhi tone just before node i is only valid if
            // the syllable at i has the right tone; if so its penalty is returned.
            let mut refund: HashMap<*const Hyp, f64> = HashMap::new();
            if i > 0 && is_sandhi_syllable(&self.syllables[i - 1]) {
                let next = &self.syllables[i];
                let prev_syl = &self.syllables[i - 1];
                hyps.retain(|h| {
                    if h.start != i - 1 {
                        return true;
                    }
                    let Some(word) = &h.word else { return true };
                    let Some(tones) = sandhi_tones(word, prev_syl) else { return true };
                    let ok = !is_literal(next) && (tones.contains(&tone_of(next)) || sandhi_also(word, prev_syl, next));
                    if ok {
                        refund.insert(Rc::as_ptr(h), -SANDHI_PENALTY);
                    }
                    ok
                });
            }
            let key: Vec<String> = self.syllables[i..j].to_vec();
            let key_strs: Vec<&str> = key.iter().map(String::as_str).collect();
            let mut words = self.candidates(&key);
            if !self.pinned.is_empty() {
                if let Some(fixed) = self.pinned_get((i, j)) {
                    words = vec![Rc::clone(fixed)];
                } else {
                    words.retain(|w| self.allowed(i, j, w));
                }
            }
            let lexicon = lexicon_rc.borrow();
            for word in &words {
                let mut bonus = 0.0f64;
                if lexicon.user_bonus != 0.0 && lexicon.is_user(word, &key_strs) {
                    bonus += lexicon.user_bonus;
                } else if let Some(b) = self.word_bias.get(&**word) {
                    bonus += b;
                }
                // A literal is scored as the token class the model was trained with;
                // a space does not touch the model.
                let literal_key = j - i == 1 && is_literal(&key[0]);
                let token: Option<Token> = if literal_key {
                    if word.chars().all(char::is_whitespace) && !word.is_empty() {
                        None
                    } else {
                        Some(self.lm.token(&MixtureLM::map_token(word, |t| lexicon.contains(t))))
                    }
                } else {
                    Some(self.lm.token(word))
                };
                if j - i == 1 {
                    if sandhi_tones(word, &key[0]).is_some() {
                        bonus += SANDHI_PENALTY;
                    } else if !literal_key {
                        // P(word, reading | history) = P(word | history) * P(reading | word).
                        bonus += lexicon.reading_logp(word, &key[0]);
                    }
                }
                for h in &hyps {
                    let mut tail = String::new();
                    if excluding {
                        match self.extend(&lexicon, &h.tail, i, word) {
                            Some(t) => tail = t,
                            None => continue,
                        }
                    }
                    let (logp, state) = match token {
                        Some(t) => self.lm.score(&h.state, t),
                        None => (0.0, Rc::clone(&h.state)),
                    };
                    let mut total = h.score + logp + self.word_penalty + bonus;
                    if !refund.is_empty() {
                        total += refund.get(&Rc::as_ptr(h)).copied().unwrap_or(0.0);
                    }
                    if let Some(extra) = &self.extra_score {
                        total += extra(h.word.as_deref(), word);
                    }
                    let origin = if self.merge { None } else { Some(Rc::as_ptr(h) as usize) };
                    let slot = Slot { state: Rc::clone(&state), tail: tail.clone(), origin };
                    merged.offer(
                        slot,
                        || Hyp {
                            score: total,
                            state,
                            word: Some(Rc::clone(word)),
                            start: i,
                            prev: Some(Rc::clone(h)),
                            tail,
                        },
                        total,
                    );
                }
            }
        }
        if merged.order.is_empty() {
            // No dictionary word ends here: pass the raw syllable through as an
            // unknown token and keep decoding.
            let raw: Word = Rc::from(self.syllables[j - 1].as_str());
            for h in &self.nodes[j - 1] {
                let (logp, state) = self.lm.score(&h.state, self.unknown_char);
                let slot = Slot { state: Rc::clone(&state), tail: String::new(), origin: None };
                merged.first(slot, || Hyp {
                    score: h.score + logp,
                    state,
                    word: Some(Rc::clone(&raw)),
                    start: j - 1,
                    prev: Some(Rc::clone(h)),
                    tail: String::new(),
                });
            }
        }
        let mut best = merged.order;
        sort_best_first(&mut best, |h| h.score);
        best.truncate(self.beam);
        self.nodes.push(best);
    }

    /// Remove the last syllable (backspace); pins reaching into it are dropped.
    pub fn pop(&mut self) {
        if !self.syllables.is_empty() {
            self.delete(self.syllables.len() - 1);
        }
    }

    fn rebuild(&mut self, keep: usize, items: Vec<String>) {
        self.syllables = items[..keep].to_vec();
        self.nodes.truncate(keep + 1);
        self.choice = None;
        for item in items.into_iter().skip(keep) {
            self.push(item);
        }
    }

    /// Insert a syllable or literal before position `pos`. Pins after it move
    /// along; a pin it would split is dropped.
    pub fn insert(&mut self, pos: usize, item: String) {
        if pos >= self.syllables.len() && self.pinned.is_empty() {
            self.push(item);
            return;
        }
        let mut keep = pos;
        let mut pinned = Vec::new();
        for ((a, b), w) in &self.pinned {
            let (a, b) = (*a, *b);
            if b <= pos {
                pinned.push(((a, b), Rc::clone(w)));
            } else if a >= pos {
                pinned.push(((a + 1, b + 1), Rc::clone(w)));
            } else {
                keep = keep.min(a);
            }
        }
        let mut items = self.syllables.clone();
        items.insert(pos.min(items.len()), item);
        self.pinned = pinned;
        self.rebuild(keep, items);
    }

    /// Remove the item at position `pos`; pins covering it are dropped.
    pub fn delete(&mut self, pos: usize) {
        if pos >= self.syllables.len() {
            return;
        }
        let mut keep = pos;
        let mut pinned = Vec::new();
        for ((a, b), w) in &self.pinned {
            let (a, b) = (*a, *b);
            if b <= pos {
                pinned.push(((a, b), Rc::clone(w)));
            } else if a > pos {
                pinned.push(((a - 1, b - 1), Rc::clone(w)));
            } else {
                keep = keep.min(a);
            }
        }
        let mut items = self.syllables.clone();
        items.remove(pos);
        self.pinned = pinned;
        self.rebuild(keep, items);
    }

    /// Remove the first `count` items (they were committed) and decode the
    /// rest again after `start_state`.
    pub fn drop_front(&mut self, count: usize, start_state: StateRef) {
        let items: Vec<String> = self.syllables[count.min(self.syllables.len())..].to_vec();
        self.pinned = self
            .pinned
            .iter()
            .filter(|((a, _), _)| *a >= count)
            .map(|((a, b), w)| ((a - count, b - count), Rc::clone(w)))
            .collect();
        self.syllables.clear();
        self.nodes = vec![vec![Self::start_hyp(start_state)]];
        for item in items {
            self.push(item);
        }
    }

    // -- results ----------------------------------------------------------------

    /// Up to n best segmentations of the whole buffer, with log scores.
    pub fn nbest(&self, mut n: usize) -> Vec<(Vec<Word>, f64)> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut final_: Vec<Rc<Hyp>> = self.nodes.last().cloned().unwrap_or_default();
        if self.eos_weight != 0.0 && final_.len() > 1 {
            // As the prototype does (its end-of-sentence code reuses the name `n`
            // for the buffer length): the list then stops at that many results.
            n = self.syllables.len();
            let w = self.eos_weight;
            let len = self.syllables.len();
            let lexicon = self.lexicon.borrow();
            let eos_token = self.lm.token("</s>");
            let mut keyed: Vec<(Rc<Hyp>, f64)> = final_
                .into_iter()
                .map(|h| {
                    let user = h.word.as_ref().is_some_and(|word| {
                        let syls: Vec<&str> = self.syllables[h.start..len].iter().map(String::as_str).collect();
                        lexicon.is_user(word, &syls)
                    });
                    let eos = if user { 0.0 } else { w * self.lm.score(&h.state, eos_token).0 };
                    let key = h.score + eos;
                    (h, key)
                })
                .collect();
            sort_best_first(&mut keyed, |k| k.1);
            final_ = keyed.into_iter().map(|k| k.0).collect();
        }
        for h in final_ {
            let mut words = Vec::new();
            let mut cur = Some(&h);
            while let Some(c) = cur {
                let Some(word) = &c.word else { break };
                words.push(Rc::clone(word));
                cur = c.prev.as_ref();
            }
            words.reverse();
            let text: String = words.iter().map(|w| &**w).collect();
            if !seen.insert(text) {
                continue;
            }
            out.push((words, h.score));
            if out.len() == n {
                break;
            }
        }
        out
    }

    /// Items one word covers: a syllable no word matched is one item although
    /// it shows several characters.
    fn word_items(&self, word: &str, at: usize) -> usize {
        if word.chars().count() > 1 && at < self.syllables.len() && word == self.syllables[at] {
            1
        } else {
            word.chars().count()
        }
    }

    /// (start, word) of the user's own words in a segmentation of the buffer.
    pub fn user_words(&self, words: &[Word]) -> Vec<(usize, Word)> {
        let lexicon = self.lexicon.borrow();
        let mut out = Vec::new();
        let mut a = 0;
        for word in words {
            let n = self.word_items(word, a);
            let end = (a + n).min(self.syllables.len());
            let syls: Vec<&str> = self.syllables[a.min(end)..end].iter().map(String::as_str).collect();
            if lexicon.is_user(word, &syls) && !out.iter().any(|(s, w): &(usize, Word)| *s == a && w == word) {
                out.push((a, Rc::clone(word)));
            }
            a += n;
        }
        out
    }

    /// The best result as (text, word start, word end) per buffer item.
    pub fn best_items(&self) -> Vec<(String, usize, usize)> {
        let mut out: Vec<(String, usize, usize)> = Vec::new();
        for word in self.best_words() {
            let a = out.len();
            if word.chars().count() > 1 && a < self.syllables.len() && *word == *self.syllables[a] {
                out.push((word.to_string(), a, a + 1));
            } else {
                let n = word.chars().count();
                out.extend(word.chars().map(|c| (c.to_string(), a, a + n)));
            }
        }
        out
    }

    /// The result to show: the re-ranker's choice if there is one, else the best path.
    pub fn best_words(&self) -> Vec<Word> {
        if let Some(choice) = &self.choice {
            return self.show(choice);
        }
        match self.nbest(1).into_iter().next() {
            Some((words, _)) => self.show(&words),
            None => Vec::new(),
        }
    }

    /// In casual style a lone particle 地 / 得 is written 的, unless the user picked it.
    pub fn show(&self, words: &[Word]) -> Vec<Word> {
        if self.style != Style::Casual {
            return words.to_vec();
        }
        let mut out = Vec::with_capacity(words.len());
        let mut pos = 0;
        for w in words {
            let n = self.word_items(w, pos);
            let particle = PARTICLE_FORMAL.contains(&&**w)
                && self.syllables.get(pos).map(String::as_str) == Some(PARTICLE_SYLLABLE)
                && self.pinned_get((pos, pos + 1)).is_none_or(|p| **p != **w);
            out.push(if particle { Rc::from(PARTICLE_CASUAL) } else { Rc::clone(w) });
            pos += n;
        }
        out
    }

    pub fn best_text(&self) -> String {
        self.best_words().iter().map(|w| &**w).collect()
    }
}
