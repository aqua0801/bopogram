//! One typing session: keystrokes in, composition state and committed text out
//! (`prototype/bopogram/session.py`; keep the two in step).
//!
//! The session owns the composition buffer, the cursor, the candidate list and
//! the left context. Committed text is only ever appended to the context; it
//! is never rewritten.
//!
//! Editing rules (as Microsoft documents for its Bopomofo IME):
//!
//!   * Bopomofo keys build a syllable at the cursor; a tone key (Space for the
//!     first tone) completes it and inserts it at the cursor.
//!   * Space with no syllable in progress inserts a space into the buffer.
//!   * Left / Right / Home / End move the cursor inside the buffer. Backspace
//!     removes the item before the cursor, Delete the one after it. While a
//!     syllable is in progress, Backspace removes its last symbol.
//!   * Symbols and other text are inserted at the cursor as typed; a symbol can
//!     be changed to a related one from the candidate list.
//!   * The candidate list refers to the word after the cursor (the last word at
//!     the end); `cycle` changes that word without opening the list.
//!   * Escape discards the syllable in progress, or else the whole buffer.
//!   * When the buffer grows past MAX_BUFFER, its first words are committed.

use std::cell::RefCell;
use std::rc::Rc;

use crate::context::context_state;
use crate::decoder::{is_literal, literal, Decoder, Style, LITERAL};
use crate::engine::Engine;
use crate::typing_log::Record;
use crate::layout::{Layout, FINALS, INITIALS, MEDIALS, TONE_FIRST};
use crate::lexicon::{Lexicon, Word, MAX_WORD_LEN};
use crate::lm::MixtureLM;
use crate::symbols::{alternatives, full_width};

pub const MAX_CONTEXT: usize = 200;
/// Items kept in the composition buffer before the front is committed.
pub const MAX_BUFFER: usize = 30;

/// One entry of the candidate list.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub word: String,
    pub start: usize,
    pub length: usize,
}

/// The last `max` characters of `text`.
fn last_chars(text: &str, max: usize) -> String {
    let n = text.chars().count();
    text.chars().skip(n.saturating_sub(max)).collect()
}

pub struct Session {
    pub engine: Rc<Engine>,
    pub decoder: Decoder,
    lexicon: Rc<RefCell<Lexicon>>,
    lm: Rc<MixtureLM>,
    layout: &'static Layout,
    pub context: String,
    /// The front end turns this off for password fields.
    pub learn: bool,
    initial: &'static str,
    medial: &'static str,
    fin: &'static str,
    /// Number of buffer items before the cursor.
    pub cursor: usize,
    /// Text committed automatically since the last `take_flushed`.
    flushed: String,
}

impl Session {
    pub fn new(engine: Rc<Engine>, layout: &'static Layout) -> Session {
        let decoder = engine.new_decoder();
        let lexicon = Rc::clone(&decoder.lexicon);
        let lm = Rc::clone(&decoder.lm);
        let mut session = Session {
            engine,
            decoder,
            lexicon,
            lm,
            layout,
            context: String::new(),
            learn: true,
            initial: "",
            medial: "",
            fin: "",
            cursor: 0,
            flushed: String::new(),
        };
        session.clear();
        session
    }

    fn context_state(&self) -> crate::lm::StateRef {
        context_state(&self.context, &self.lexicon.borrow(), &self.lm)
    }

    fn clear(&mut self) {
        self.initial = "";
        self.medial = "";
        self.fin = "";
        self.cursor = 0;
        let state = self.context_state();
        self.decoder.reset(Some(state));
    }

    fn size(&self) -> usize {
        self.decoder.syllables.len()
    }

    // -- input ------------------------------------------------------------------

    /// Text before the cursor, as reported by the application. Only used while idle.
    pub fn set_context(&mut self, text: &str) {
        self.context = last_chars(text, MAX_CONTEXT);
        if self.size() == 0 && self.pending().is_empty() {
            self.clear();
        }
    }

    /// Bopomofo symbols of the syllable being typed.
    pub fn pending(&self) -> String {
        format!("{}{}{}", self.initial, self.medial, self.fin)
    }

    /// Model re-scoring after the buffer changed (the character model, later).
    fn rescore(&mut self) {}

    /// If the user's dictionary changed, decode the buffer again with it. Called
    /// before anything that changes or commits the buffer, so that a change
    /// shows on the next key and never moves the text under the user's eyes.
    fn sync(&mut self) {
        if self.decoder.refresh() {
            self.rescore();
        }
    }

    fn insert(&mut self, item: String) {
        self.sync();
        self.decoder.insert(self.cursor, item);
        self.cursor += 1;
        self.rescore();
        self.limit();
    }

    /// Feed one layout key. False if the key is not handled (not part of the
    /// layout, or Space with nothing being composed).
    pub fn key(&mut self, key: &str) -> bool {
        let lower = key.to_lowercase();
        let mut chars = lower.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else { return false };
        let Some(sym) = self.layout.symbol(ch) else { return false };
        if INITIALS.contains(sym) {
            self.initial = sym;
        } else if MEDIALS.contains(sym) {
            self.medial = sym;
        } else if FINALS.contains(sym) {
            self.fin = sym;
        } else {
            let body = self.pending();
            if body.is_empty() {
                if sym == TONE_FIRST && self.size() > 0 {
                    self.insert(literal(" ")); // Space is a space when no syllable is in progress
                    return true;
                }
                return false;
            }
            let syllable = match sym {
                TONE_FIRST => body,
                "˙" => format!("˙{body}"),
                tone => format!("{body}{tone}"),
            };
            self.initial = "";
            self.medial = "";
            self.fin = "";
            self.insert(syllable);
        }
        true
    }

    /// Insert characters that are not converted (letters, digits, symbols).
    pub fn text(&mut self, text: &str) -> bool {
        if !self.pending().is_empty() || text.is_empty() {
            return false;
        }
        for ch in text.chars() {
            self.insert(literal(&ch.to_string()));
        }
        true
    }

    /// Insert the full-width form of an ASCII symbol key.
    pub fn symbol(&mut self, key: &str) -> bool {
        match full_width(key) {
            Some(wide) => self.text(wide.as_str()),
            None => false,
        }
    }

    pub fn backspace(&mut self) {
        if !self.fin.is_empty() {
            self.fin = "";
        } else if !self.medial.is_empty() {
            self.medial = "";
        } else if !self.initial.is_empty() {
            self.initial = "";
        } else if self.cursor > 0 {
            self.sync();
            self.decoder.delete(self.cursor - 1);
            self.cursor -= 1;
            self.rescore();
        }
    }

    /// Remove the item after the cursor (the Delete key).
    pub fn delete(&mut self) {
        if self.pending().is_empty() && self.cursor < self.size() {
            self.sync();
            self.decoder.delete(self.cursor);
            self.rescore();
        }
    }

    pub fn move_by(&mut self, delta: i64) {
        if self.pending().is_empty() {
            self.cursor = (self.cursor as i64 + delta).clamp(0, self.size() as i64) as usize;
        }
    }

    /// Home (`end` false) or End (`end` true).
    pub fn move_to(&mut self, end: bool) {
        if self.pending().is_empty() {
            self.cursor = if end { self.size() } else { 0 };
        }
    }

    pub fn escape(&mut self) {
        if !self.pending().is_empty() {
            self.initial = "";
            self.medial = "";
            self.fin = "";
        } else {
            self.clear();
        }
    }

    /// "casual" or "formal" (see `Style`); the buffer is decoded again.
    pub fn set_style(&mut self, name: &str) -> bool {
        let Some(style) = Style::parse(name) else { return false };
        self.decoder.set_style(style);
        self.rescore();
        true
    }

    // -- candidates ---------------------------------------------------------------

    /// The word the candidate list refers to: the one holding the character
    /// after the cursor (the last one when the cursor is at the end). Inside the
    /// buffer the list starts at the character after the cursor, even in the
    /// middle of a word: with 跑道 and the cursor before 道 it offers 道.
    fn span_at_cursor(&self, items: &[(String, usize, usize)]) -> Option<(usize, usize)> {
        let size = self.size();
        if size == 0 {
            return None;
        }
        let target = self.cursor.min(size - 1);
        let &(_, mut start, end) = items.get(target)?;
        if self.cursor < size {
            start = target;
        }
        Some((start, end))
    }

    /// Words that can start where the word at the cursor starts, longest first.
    pub fn candidates(&self) -> Vec<Candidate> {
        let items = self.decoder.best_items();
        let Some((start, _)) = self.span_at_cursor(&items) else { return Vec::new() };
        let syls = &self.decoder.syllables;
        if is_literal(&syls[start]) {
            // A symbol can be changed to a related symbol.
            let ch = &syls[start][LITERAL.len_utf8()..];
            return alternatives(ch).into_iter().map(|alt| Candidate { word: alt, start, length: 1 }).collect();
        }
        let lexicon = self.lexicon.borrow();
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for length in (1..=MAX_WORD_LEN.min(syls.len() - start)).rev() {
            let key: Vec<&str> = syls[start..start + length].iter().map(String::as_str).collect();
            // Single characters: frequent under THIS reading first. The user's own words first.
            let mut ranked: Vec<(Word, bool, f64)> = lexicon
                .candidates(&key)
                .into_iter()
                .map(|w| {
                    let mut score = self.lm.unigram(&w);
                    if length == 1 {
                        score += lexicon.reading_logp(&w, key[0]);
                    }
                    let user = lexicon.is_user(&w, &key);
                    (w, user, score)
                })
                .collect();
            // Descending and stable, as Python's sorted(reverse=True).
            ranked.sort_by(|a, b| (b.1, b.2).partial_cmp(&(a.1, a.2)).unwrap_or(std::cmp::Ordering::Equal));
            for (word, _, _) in ranked {
                if seen.insert(Rc::clone(&word)) {
                    out.push(Candidate { word: word.to_string(), start, length });
                }
            }
        }
        out
    }

    /// Pick candidate `index`; that span is fixed and the rest is decoded again.
    pub fn select(&mut self, index: i64) -> bool {
        let cands = self.candidates();
        if index < 0 || index as usize >= cands.len() {
            return false;
        }
        self.sync();
        let c = &cands[index as usize];
        if is_literal(&self.decoder.syllables[c.start]) {
            self.decoder.delete(c.start);
            self.decoder.insert(c.start, literal(&c.word));
            self.rescore();
            return true;
        }
        let span = (c.start, c.start + c.length);
        let mut pinned: Vec<((usize, usize), Word)> = self
            .decoder
            .pinned
            .iter()
            .filter(|((a, b), _)| *b <= span.0 || *a >= span.1)
            .cloned()
            .collect();
        pinned.push((span, Rc::from(c.word.as_str())));
        self.decoder.repin(pinned, None);
        self.cursor = self.cursor.max(self.size().min(span.1));
        true
    }

    /// Change the word at the cursor to its next (or previous) candidate of the
    /// same length, without opening the candidate list.
    pub fn cycle(&mut self, delta: i64) -> bool {
        self.sync();
        let items = self.decoder.best_items();
        let Some((a, b)) = self.span_at_cursor(&items) else { return false };
        let current: String = items[a..b].iter().map(|t| t.0.as_str()).collect();
        let cands = self.candidates();
        let same: Vec<usize> = (0..cands.len()).filter(|&i| cands[i].length == b - a).collect();
        if same.len() < 2 {
            return false;
        }
        let at = same.iter().position(|&i| cands[i].word == current).map_or(-1, |k| k as i64);
        let next = (at + delta).rem_euclid(same.len() as i64) as usize;
        self.select(same[next] as i64)
    }

    // -- output -------------------------------------------------------------------

    /// Learn the words covering the first `count` items. Runs are cut at
    /// literals, unconverted syllables and excluded text; words holding a pick
    /// count as corrections.
    fn learn_items(&self, count: usize) {
        let Some(user) = self.engine.user.as_ref().filter(|_| self.learn) else { return };
        let mut user = user.borrow_mut();
        let lexicon = self.lexicon.borrow();
        let buf = &self.decoder.syllables;
        let items = self.decoder.best_items();
        let items = &items[..count.min(items.len())];
        let mut run: Vec<String> = Vec::new();
        let mut picked: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut last = None;
        for (text, a, b) in items {
            let (a, b) = (*a, *b);
            if last == Some((a, b)) {
                continue;
            }
            last = Some((a, b));
            let word: String = items[a.min(items.len())..b.min(items.len())].iter().map(|t| t.0.as_str()).collect();
            let syls: Vec<&str> = buf[a..b.min(buf.len())].iter().map(String::as_str).collect();
            if b > count
                || is_literal(&buf[a])
                || (b - a == 1 && text.chars().count() > 1)
                || !lexicon.contains(&word)
                || lexicon.blocked(&word, &syls)
            {
                if !run.is_empty() {
                    user.learn(&run, &picked);
                }
                run.clear();
                picked.clear();
                continue;
            }
            run.push(word);
            if self.decoder.pinned.iter().any(|((p, q), _)| a <= *p && *q <= b) {
                picked.insert(run.len() - 1);
            }
        }
        if !run.is_empty() {
            user.learn(&run, &picked);
        }
        user.save(false);
    }

    /// Write the first `count` items to the typing log (if enabled and learning is on).
    fn record(&self, count: usize, auto: bool) {
        let Some(log) = self.engine.typing_log.as_ref() else { return };
        if !self.learn || count == 0 {
            return;
        }
        let dec = &self.decoder;
        let texts: Vec<String> = dec.best_items().into_iter().map(|t| t.0).collect();
        let first: Vec<String> = if dec.pinned.is_empty() {
            texts.clone()
        } else {
            // What the engine would have chosen without the user's picks.
            let mut plain = self.engine.new_decoder();
            plain.style = dec.style;
            plain.reset(Some(dec.start_state()));
            for item in &dec.syllables {
                plain.push(item.clone());
            }
            plain.best_items().into_iter().map(|t| t.0).collect()
        };
        let mut picked: Vec<(usize, usize, String)> = dec
            .pinned
            .iter()
            .filter(|((_, b), _)| *b <= count)
            .map(|((a, b), w)| (*a, b - a, w.to_string()))
            .collect();
        picked.sort();
        let join = |t: &[String]| t[..count.min(t.len())].concat();
        log.write(&Record {
            items: &dec.syllables[..count.min(dec.syllables.len())],
            first: &join(&first),
            text: &join(&texts),
            picked: &picked,
            context: &self.context,
            style: dec.style.name(),
            auto,
        });
    }

    /// Commit the first words when the buffer is too long. Committed text is final.
    fn limit(&mut self) {
        if self.size() <= MAX_BUFFER {
            return;
        }
        let items = self.decoder.best_items();
        let mut count = 0;
        while count < items.len() && self.size() - count > MAX_BUFFER && items[count].2 <= self.cursor {
            count = items[count].2; // whole words only, and never past the cursor
        }
        if count == 0 {
            return;
        }
        self.learn_items(count);
        self.record(count, true);
        let text: String = items[..count].iter().map(|t| t.0.as_str()).collect();
        self.flushed.push_str(&text);
        self.context = last_chars(&(self.context.clone() + &text), MAX_CONTEXT);
        let state = self.context_state();
        self.decoder.drop_front(count, state);
        self.cursor -= count;
        self.rescore();
    }

    /// Finish the composition and return its text (a syllable in progress is dropped).
    pub fn commit(&mut self) -> String {
        self.sync();
        let text = self.decoder.best_text();
        if !text.is_empty() {
            self.learn_items(self.size());
            self.record(self.size(), false);
        }
        self.context = last_chars(&(self.context.clone() + &text), MAX_CONTEXT);
        self.clear();
        text
    }

    /// Text committed automatically since the last call (see MAX_BUFFER).
    pub fn take_flushed(&mut self) -> String {
        std::mem::take(&mut self.flushed)
    }

    pub fn state(&self) -> State {
        let items = self.decoder.best_items();
        let pending = self.pending();
        let before: String = items[..self.cursor.min(items.len())].iter().map(|t| t.0.as_str()).collect();
        let after: String = items[self.cursor.min(items.len())..].iter().map(|t| t.0.as_str()).collect();
        State {
            preedit: format!("{before}{pending}{after}"),
            caret: before.chars().count() + pending.chars().count(),
            pending,
            cursor: self.cursor,
            syllables: self.size(),
            segments: self.decoder.best_words().iter().map(|w| w.to_string()).collect(),
            style: self.decoder.style,
        }
    }
}

/// The composition state every response carries.
pub struct State {
    pub preedit: String,
    pub pending: String,
    pub cursor: usize,
    /// Characters before the caret in `preedit`.
    pub caret: usize,
    pub syllables: usize,
    pub segments: Vec<String>,
    pub style: Style,
}
