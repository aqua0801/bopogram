//! Language model: a weighted mixture of n-gram models.
//!
//! Scores are natural-log probabilities. The mixture is a linear
//! interpolation, P(w | h) = sum_i weight_i * P_i(w | h), so each hypothesis
//! carries one state per component model (`prototype/bopogram/lm.py`).
//!
//! Tokens are strings; each is given one id for all the models (`Token`),
//! which stands for the word's id in every model (`<unk>` where a model does
//! not know it).

pub mod bits;
pub mod build;
pub mod model;

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;

pub use model::{Model, State};

pub const NUM: &str = "<n>";
pub const LATIN: &str = "<e>";
pub const UNKNOWN_CHAR: &str = "<x>";
pub const PUNCT: &str = "<p>";

/// Characters written in another standard form in the training text.
const PREFERRED_FORMS: [(char, char); 15] = [
    ('臺', '台'),
    ('裏', '裡'),
    ('爲', '為'),
    ('啓', '啟'),
    ('綫', '線'),
    ('衞', '衛'),
    ('峯', '峰'),
    ('羣', '群'),
    ('敍', '敘'),
    ('説', '說'),
    ('産', '產'),
    ('奬', '獎'),
    ('滙', '匯'),
    ('恒', '恆'),
    ('麽', '麼'),
];

/// The states of all the component models: what the mixture remembers.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct MixState(pub Box<[State]>);

pub type StateRef = Rc<MixState>;

/// A token as all the models know it.
pub type Token = u32;

pub struct MixtureLM {
    models: Vec<Model>,
    weights: Vec<f64>,
    /// Token -> the word's id in each model.
    ids: RefCell<Vec<Box<[u32]>>>,
    tokens: RefCell<HashMap<String, Token>>,
    cache: RefCell<HashMap<(StateRef, Token), (f64, StateRef)>>,
    unigrams: RefCell<HashMap<String, f64>>,
    null: StateRef,
    context_free: std::cell::Cell<bool>,
}

const LN_10: f64 = std::f64::consts::LN_10;
const CACHE_LIMIT: usize = 300_000;
const UNIGRAM_LIMIT: usize = 500_000;

impl MixtureLM {
    pub fn load(paths: &[PathBuf], weights: &[f64]) -> io::Result<MixtureLM> {
        if paths.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "at least one model is required"));
        }
        if paths.len() != weights.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "one weight per model is required"));
        }
        let models = paths
            .iter()
            .map(|p| Model::load(p).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", p.display()))))
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Self::new(models, weights))
    }

    pub fn new(models: Vec<Model>, weights: &[f64]) -> MixtureLM {
        let total: f64 = weights.iter().sum();
        let weights = weights.iter().map(|w| w / total).collect();
        let null = Rc::new(MixState(vec![State::EMPTY; models.len()].into_boxed_slice()));
        MixtureLM {
            models,
            weights,
            ids: RefCell::new(Vec::new()),
            tokens: RefCell::new(HashMap::new()),
            cache: RefCell::new(HashMap::new()),
            unigrams: RefCell::new(HashMap::new()),
            null,
            context_free: std::cell::Cell::new(false),
        }
    }

    pub fn memory(&self) -> usize {
        self.models.iter().map(Model::memory).sum()
    }

    /// The id of `word` for all the models.
    pub fn token(&self, word: &str) -> Token {
        if let Some(&t) = self.tokens.borrow().get(word) {
            return t;
        }
        let per_model: Box<[u32]> = self.models.iter().map(|m| m.id(word).unwrap_or(model::UNK)).collect();
        let mut ids = self.ids.borrow_mut();
        let t = ids.len() as Token;
        ids.push(per_model);
        self.tokens.borrow_mut().insert(word.to_owned(), t);
        t
    }

    /// The state at the start of a sentence.
    pub fn begin(&self) -> StateRef {
        if self.context_free.get() {
            return Rc::clone(&self.null);
        }
        let s = self.token("<s>");
        let ids = self.ids.borrow();
        let states = self.models.iter().zip(ids[s as usize].iter()).map(|(m, &id)| m.begin_sentence(id)).collect();
        Rc::new(MixState(states))
    }

    /// Score every word without context from now on (the `word-unigram`
    /// baseline of `eval`): `score` gives the unigram score and one state.
    pub fn set_context_free(&self) {
        self.context_free.set(true);
    }

    /// ln P(token | state) and the following state.
    pub fn score(&self, state: &StateRef, token: Token) -> (f64, StateRef) {
        if self.context_free.get() {
            return (self.score_in_context(&self.null.clone(), token).0, Rc::clone(&self.null));
        }
        self.score_in_context(state, token)
    }

    fn score_in_context(&self, state: &StateRef, token: Token) -> (f64, StateRef) {
        if let Some(hit) = self.cache.borrow().get(&(Rc::clone(state), token)) {
            return (hit.0, Rc::clone(&hit.1));
        }
        let ids = self.ids.borrow();
        let ids = &ids[token as usize];
        let mut out = Vec::with_capacity(self.models.len());
        let logp = if self.models.len() == 1 {
            let (s, o) = self.models[0].score(&state.0[0], ids[0]);
            out.push(o);
            s as f64 * LN_10
        } else {
            let mut p = 0.0f64;
            for (k, model) in self.models.iter().enumerate() {
                let (s, o) = model.score(&state.0[k], ids[k]);
                p += self.weights[k] * 10f64.powf(s as f64);
                out.push(o);
            }
            if p > 0.0 {
                p.ln()
            } else {
                -99.0
            }
        };
        let next = Rc::new(MixState(out.into_boxed_slice()));
        let mut cache = self.cache.borrow_mut();
        if cache.len() > CACHE_LIMIT {
            cache.clear();
        }
        cache.insert((Rc::clone(state), token), (logp, Rc::clone(&next)));
        (logp, next)
    }

    pub fn score_word(&self, state: &StateRef, word: &str) -> (f64, StateRef) {
        self.score(state, self.token(word))
    }

    /// Context-free ln P(word); used to rank candidates and to segment context.
    pub fn unigram(&self, word: &str) -> f64 {
        if let Some(&v) = self.unigrams.borrow().get(word) {
            return v;
        }
        let v = self.score_in_context(&self.null.clone(), self.token(word)).0;
        let mut unigrams = self.unigrams.borrow_mut();
        if unigrams.len() > UNIGRAM_LIMIT {
            unigrams.clear();
        }
        unigrams.insert(word.to_owned(), v);
        v
    }

    /// The token the models were trained with for `tok`: the word itself if
    /// `in_vocab` knows it, else its class (number, Latin, unknown Chinese
    /// character, punctuation).
    pub fn map_token(tok: &str, in_vocab: impl Fn(&str) -> bool) -> String {
        let tok: String = tok
            .chars()
            .map(|c| PREFERRED_FORMS.iter().find(|(from, _)| *from == c).map_or(c, |(_, to)| *to))
            .collect();
        if in_vocab(&tok) {
            return tok;
        }
        let mut chars = tok.chars();
        let first = chars.next();
        let rest_ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '%' | '+' | '-');
        if first.is_some_and(|c| c.is_ascii_digit()) && tok.chars().skip(1).all(|c| c.is_ascii_digit() || matches!(c, '.' | '%' | '+' | '-')) {
            return NUM.to_owned();
        }
        if first.is_some_and(|c| c.is_ascii_alphanumeric()) && tok.chars().skip(1).all(rest_ok) {
            return LATIN.to_owned();
        }
        if first.is_some() && tok.chars().all(is_cjk) {
            return UNKNOWN_CHAR.to_owned();
        }
        PUNCT.to_owned()
    }
}

/// The Chinese characters the models and the context treat as words:
/// U+4E00..U+9FFF and U+3400..U+4DBF.
pub fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3400}'..='\u{4dbf}').contains(&c)
}
