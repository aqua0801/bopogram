//! Left context: text already committed before the composition buffer
//! (`prototype/bopogram/context.py`).
//!
//! The context is read-only. It is segmented into dictionary words and fed
//! to the language model so that the first words of the composition are
//! scored in context; committed text is never rewritten.

use crate::lexicon::{Lexicon, MAX_WORD_LEN};
use crate::lm::{is_cjk, MixtureLM, StateRef, PUNCT};

/// An n-gram model only looks a few words back.
pub const MAX_CONTEXT_CHARS: usize = 40;

fn is_sentence_end(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '!' | '?' | '\n')
}

fn latin_rest(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '%' | '+' | '-')
}

/// The pieces of `text`: runs of Chinese characters, runs of Latin letters
/// and digits, and single other characters; white space separates and is dropped.
fn pieces(text: &str) -> Vec<(bool, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if is_cjk(c) {
            let start = i;
            while i < chars.len() && is_cjk(chars[i]) {
                i += 1;
            }
            out.push((true, chars[start..i].iter().collect()));
        } else if c.is_ascii_alphanumeric() {
            let start = i;
            i += 1;
            while i < chars.len() && latin_rest(chars[i]) {
                i += 1;
            }
            out.push((false, chars[start..i].iter().collect()));
        } else {
            if !c.is_whitespace() {
                out.push((false, c.to_string()));
            }
            i += 1;
        }
    }
    out
}

/// Unigram-Viterbi segmentation into dictionary words (other runs kept whole).
pub fn segment(text: &str, lexicon: &Lexicon, lm: &MixtureLM) -> Vec<String> {
    let mut tokens = Vec::new();
    for (chinese, piece) in pieces(text) {
        if !chinese {
            tokens.push(piece);
            continue;
        }
        let run: Vec<char> = piece.chars().collect();
        let n = run.len();
        let mut best = vec![f64::NEG_INFINITY; n + 1];
        best[0] = 0.0;
        let mut back = vec![0usize; n + 1];
        for i in 0..n {
            if best[i] == f64::NEG_INFINITY {
                continue;
            }
            for j in i + 1..=n.min(i + MAX_WORD_LEN) {
                let w: String = run[i..j].iter().collect();
                let known = lexicon.contains(&w);
                if j - i > 1 && !known {
                    continue;
                }
                let s = best[i] + if known { lm.unigram(&w) } else { -30.0 };
                if s > best[j] {
                    best[j] = s;
                    back[j] = i;
                }
            }
        }
        let mut cut = Vec::new();
        let mut j = n;
        while j > 0 {
            cut.push(run[back[j]..j].iter().collect::<String>());
            j = back[j];
        }
        cut.reverse();
        tokens.extend(cut);
    }
    tokens
}

/// The model state after reading the context (the current clause only).
pub fn context_state(text: &str, lexicon: &Lexicon, lm: &MixtureLM) -> StateRef {
    let mut state = lm.begin();
    if text.is_empty() {
        return state;
    }
    let chars: Vec<char> = text.chars().collect();
    let last: String = chars[chars.len().saturating_sub(MAX_CONTEXT_CHARS)..].iter().collect();
    let tail = last.rsplit(is_sentence_end).next().unwrap_or("");
    let mut prev: Option<String> = None;
    for tok in segment(tail, lexicon, lm) {
        let mapped = MixtureLM::map_token(&tok, |t| lexicon.contains(t));
        if mapped == PUNCT && prev.as_deref().is_none_or(|p| p == PUNCT) {
            continue;
        }
        state = lm.score_word(&state, &mapped).1;
        prev = Some(mapped);
    }
    state
}
