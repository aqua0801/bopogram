//! Engine: configuration, loading, and one-call conversion
//! (`prototype/bopogram/engine.py`).
//!
//! The config is the runtime bundle's `config.json`, shared with the Python
//! engine. Model paths there name KenLM files (`*.binary`); this engine reads
//! its own format, from the file of the same name with the extension `.bglm`
//! (built by `bopogram build-lm`), beside the config's file or in the folder
//! given with `--models`. A path that already ends in `.bglm` is used as is.
//!
//! The character model ("charlm") likewise runs from our format, `charlm.bin`
//! (exported by `training/neural/export_charlm_native.py`), found in the
//! `--models` folder or beside the model file the config names; no ONNX Runtime.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::Value;

use crate::charlm::CharLM;
use crate::context::context_state;
use crate::decoder::{Decoder, ExtraScore, Style};
use crate::lexicon::Lexicon;
use crate::lm::MixtureLM;
use crate::typing_log::TypingLog;
use crate::user::UserModel;
use crate::userdict::UserDict;

/// The character model and how it re-ranks ("charlm" in the config).
pub struct Rerank {
    pub lm: RefCell<CharLM>,
    pub weight: f64,
    pub top: usize,
}

impl Rerank {
    /// Pick among the decoder's `top` best results (`Engine.rescore` in engine.py).
    pub fn rescore(&self, decoder: &mut Decoder, context: &str) {
        if decoder.syllables.is_empty() {
            return;
        }
        let mut hyps = decoder.nbest(self.top);
        // The character model does not know the user's words and must not overrule
        // them: it only chooses among results that keep those of the best path.
        if hyps.len() > 1 && decoder.lexicon.borrow().is_user_active() {
            let keep = decoder.user_words(&hyps[0].0);
            if !keep.is_empty() {
                hyps.retain(|(words, _)| {
                    let have = decoder.user_words(words);
                    keep.iter().all(|k| have.contains(k))
                });
            }
        }
        if hyps.len() > 1 {
            let texts: Vec<String> = hyps.iter().map(|(words, _)| words.iter().map(|w| &**w).collect()).collect();
            let lm = self.lm.borrow_mut().score(context, &texts);
            let total = |i: usize| hyps[i].1 + self.weight * lm[i];
            let mut best = 0;
            for i in 1..hyps.len() {
                if total(i) > total(best) {
                    best = i;
                }
            }
            // Formal style: 的 / 地 / 得 stay as the n-gram model has them. The character
            // model learned from blogs, which write 的 for all three; in casual style the
            // three are shown as 的 anyway.
            if decoder.style == Style::Formal && only_de(&texts[0], &texts[best]) {
                best = 0;
            }
            decoder.choice = Some(hyps.swap_remove(best).0);
        }
    }
}

/// `b` differs from `a` only where both have one of 的 / 地 / 得.
fn only_de(a: &str, b: &str) -> bool {
    let de = |c: char| matches!(c, '的' | '地' | '得');
    a != b && a.chars().count() == b.chars().count() && a.chars().zip(b.chars()).all(|(x, y)| x == y || (de(x) && de(y)))
}

#[cfg(test)]
mod only_de_tests {
    use super::only_de;

    #[test]
    fn differs_only_in_de() {
        assert!(only_de("慢慢地走", "慢慢的走"));
        assert!(only_de("說得好的", "說的好地"));
        assert!(!only_de("慢慢地走", "慢慢地走")); // the same text
        assert!(!only_de("慢慢地走", "曼曼的走")); // differs elsewhere too
        assert!(!only_de("地走", "的走路"));
    }
}

/// The character model named by the config: our format (`charlm.bin`) as
/// given, else `charlm.bin` in `models` or beside the file named; its
/// `charlm.json` beside it.
fn load_charlm(cfg: &Value, base: &Path, models: Option<&Path>) -> io::Result<Option<Rerank>> {
    let Some(c) = cfg.get("charlm").filter(|c| !c.is_null()) else { return Ok(None) };
    let model = c.get("model").and_then(Value::as_str).ok_or_else(|| invalid("config: \"charlm\" without \"model\"".into()))?;
    // Normally only in the model's table (its tier's tuned values); given here,
    // they override it (for experiments).
    let weight = c.get("weight").and_then(Value::as_f64);
    let top = c.get("top").and_then(Value::as_u64).map(|v| v as usize);
    load_rerank(&base.join(model), models, weight, top)
}

/// A character model from a model path as `load_charlm` finds it; `None`
/// (with a warning) if there is no `charlm.bin` for it. `weight` and `top`
/// as given, else the model's own (`rerank` in its table), else 0.5 and 5.
pub fn load_rerank(named: &Path, models: Option<&Path>, weight: Option<f64>, top: Option<usize>) -> io::Result<Option<Rerank>> {
    let named = named.to_path_buf();
    let candidates: Vec<PathBuf> = if named.extension().is_some_and(|e| e == "bin") {
        vec![named.clone()]
    } else {
        models.map(|m| m.join("charlm.bin")).into_iter().chain([named.with_file_name("charlm.bin")]).collect()
    };
    let Some(file) = candidates.into_iter().find(|p| p.exists()) else {
        eprintln!(
            "warning: no charlm.bin for {}; the character model is off (export it with training/neural/export_charlm_native.py)",
            named.display()
        );
        return Ok(None);
    };
    let mut meta = crate::charlm::table_path(&file);
    if !meta.exists() {
        meta = crate::charlm::table_path(&named);
    }
    let lm = CharLM::load(&file, &meta)?;
    let weight = weight.or(lm.rerank_weight).unwrap_or(0.5);
    let top = top.or(lm.rerank_top).unwrap_or(5);
    Ok(Some(Rerank { lm: RefCell::new(lm), weight, top }))
}

/// The user model's bonus as the decoder's extra score.
fn extra_score(user: &Option<Rc<RefCell<UserModel>>>) -> Option<ExtraScore> {
    let user = Rc::clone(user.as_ref()?);
    Some(Rc::new(move |prev: Option<&str>, word: &str| user.borrow().bonus(prev, word)))
}

pub fn default_word_bias() -> HashMap<String, f64> {
    // The models pick 她 / 它 where 他 is meant about as often as the reverse;
    // a small handicap makes 他 the default (see engine.py).
    [("她".to_owned(), -1.0), ("它".to_owned(), -1.0)].into_iter().collect()
}

/// The config file: explicit, else $BOPOGRAM_CONFIG, else ~/bopogram-data/models/config.json.
pub fn config_path(explicit: Option<&str>) -> PathBuf {
    if let Some(p) = explicit {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("BOPOGRAM_CONFIG") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).unwrap_or_default();
    PathBuf::from(home).join("bopogram-data").join("models").join("config.json")
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn read_config(path: &Path) -> io::Result<Value> {
    let text = std::fs::read_to_string(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| invalid(format!("{}: {e}", path.display())))
}

/// The dictionary named by a config.
pub fn load_lexicon(config: &Value, base: &Path) -> io::Result<Lexicon> {
    let dict = config.get("dict").and_then(Value::as_str).ok_or_else(|| invalid("config: no \"dict\"".into()))?;
    Lexicon::load(&base.join(dict))
}

/// Our model file for a model path in the config: the same name with the
/// extension `.bglm`, beside it or in `models` if given (so the runtime bundle
/// itself need not be written to).
pub fn model_file(path: &Path, models: Option<&Path>) -> PathBuf {
    let file = if path.extension().is_some_and(|e| e == "bglm") { path.to_path_buf() } else { path.with_extension("bglm") };
    match (models, file.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => file,
    }
}

/// True if the model the config names (a KenLM file) was changed after our
/// file was built from it: our file then no longer gives the same scores.
fn stale(source: &Path, ours: &Path) -> bool {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    source != ours && matches!((modified(source), modified(ours)), (Some(s), Some(o)) if s > o)
}

/// Added to the score of every user word unless the config says otherwise.
pub const USER_DICT_BONUS: f64 = 6.0;

/// A path from the config: `~` is the home directory, relative paths are
/// relative to the config's folder.
fn user_path(value: &str, base: &Path) -> PathBuf {
    let home = || std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    let path = match value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
        Some(rest) => home().map_or_else(|| PathBuf::from(value), |h| h.join(rest)),
        None if value == "~" => home().unwrap_or_else(|| PathBuf::from(value)),
        None => PathBuf::from(value),
    };
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

pub struct Engine {
    pub lexicon: Rc<RefCell<Lexicon>>,
    pub lm: Rc<MixtureLM>,
    pub decoder: Decoder,
    /// The user's dictionary (words and exclusions), if enabled.
    pub userdict: Option<RefCell<UserDict>>,
    /// Personal learning, if enabled.
    pub user: Option<Rc<RefCell<UserModel>>>,
    pub typing_log: Option<TypingLog>,
    beam: usize,
    max_candidates: usize,
    word_penalty: f64,
    eos_weight: f64,
    style: Style,
    word_bias: Rc<HashMap<String, f64>>,
    /// The character model, if the config names one.
    pub charlm: Option<Rc<Rerank>>,
}

impl Engine {
    /// `use_user_model` false leaves out everything personal (user model, user
    /// dictionary, typing log), as one-off conversions do. `models` is the
    /// folder with our model files when they are not beside the config's.
    pub fn load(config_file: &Path, use_user_model: bool, model_dir: Option<&Path>) -> io::Result<Engine> {
        let cfg = read_config(config_file)?;
        let base = config_file.parent().unwrap_or(Path::new("."));
        let mut lexicon = load_lexicon(&cfg, base)?;
        if let Some(prior) = cfg.get("reading_prior").and_then(Value::as_str).filter(|p| !p.is_empty()) {
            let weight = cfg.get("reading_prior_weight").and_then(Value::as_f64).unwrap_or(1.0);
            lexicon.load_reading_prior(&base.join(prior), weight)?;
        }
        let models = cfg.get("models").and_then(Value::as_array).ok_or_else(|| invalid("config: no \"models\"".into()))?;
        let mut paths = Vec::new();
        let mut weights = Vec::new();
        for m in models {
            let path = m.get("path").and_then(Value::as_str).ok_or_else(|| invalid("config: a model without \"path\"".into()))?;
            let source = base.join(path);
            let file = model_file(&source, model_dir);
            if !file.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{}: missing; build it from the model's ARPA file with `bopogram build-lm`", file.display()),
                ));
            }
            if stale(&source, &file) {
                eprintln!(
                    "warning: {} is newer than {}; build it again from the new ARPA file (`bopogram build-lm`)",
                    source.display(),
                    file.display()
                );
            }
            paths.push(file);
            weights.push(m.get("weight").and_then(Value::as_f64).unwrap_or(1.0));
        }
        let lm = Rc::new(MixtureLM::load(&paths, &weights)?);
        let style = match cfg.get("style").and_then(Value::as_str) {
            None => Style::Casual,
            Some(name) => Style::parse(name).ok_or_else(|| invalid(format!("unknown style {name:?}")))?,
        };
        let word_bias = match cfg.get("word_bias") {
            Some(Value::Object(map)) => map.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?))).collect(),
            _ => default_word_bias(),
        };
        let text_of = |name: &str| cfg.get(name).and_then(Value::as_str).filter(|v| !v.is_empty());
        let mut user = None;
        let mut userdict = None;
        let mut typing_log = None;
        if use_user_model {
            if let Some(p) = text_of("user_model") {
                user = Some(Rc::new(RefCell::new(UserModel::load(Some(user_path(p, base)))?)));
            }
            // Each defaults to the path shown; null (or "") turns it off.
            let file = |name: &str, default: &str| match cfg.get(name) {
                None => Some(user_path(default, base)),
                Some(Value::String(s)) if !s.is_empty() => Some(user_path(s, base)),
                _ => None,
            };
            let files = (file("user_dict", "userdata/userdict.tsv"), file("excluded_words", "userdata/excluded.tsv"));
            if files.0.is_some() || files.1.is_some() {
                let ud = UserDict::new(files.0, files.1);
                lexicon.user_bonus = cfg.get("user_dict_bonus").and_then(Value::as_f64).unwrap_or(USER_DICT_BONUS);
                lexicon.refresh(Some(&ud));
                userdict = Some(RefCell::new(ud));
            }
            if let Some(p) = text_of("typing_log") {
                typing_log = Some(TypingLog::new(user_path(p, base)));
            }
        }
        let lexicon = Rc::new(RefCell::new(lexicon));
        let get_usize = |name: &str, default: usize| cfg.get(name).and_then(Value::as_u64).map_or(default, |v| v as usize);
        let get_f64 = |name: &str| cfg.get(name).and_then(Value::as_f64).unwrap_or(0.0);
        let (beam, max_candidates) = (get_usize("beam", 16), get_usize("max_candidates", 40));
        let (word_penalty, eos_weight) = (get_f64("word_penalty"), get_f64("eos_weight"));
        let word_bias = Rc::new(word_bias);
        let charlm = load_charlm(&cfg, base, model_dir)?.map(Rc::new);
        let mut decoder = Decoder::new(
            Rc::clone(&lexicon),
            Rc::clone(&lm),
            beam,
            max_candidates,
            extra_score(&user),
            word_penalty,
            eos_weight,
            style,
            Rc::clone(&word_bias),
        );
        // Merging hypotheses with equal n-gram state leaves near-identical results in
        // nbest(); without merging the top few are real alternatives worth re-ranking.
        decoder.merge = charlm.is_none();
        Ok(Engine {
            lexicon,
            lm,
            decoder,
            userdict,
            user,
            typing_log,
            beam,
            max_candidates,
            word_penalty,
            eos_weight,
            style,
            word_bias,
            charlm,
        })
    }

    pub fn max_candidates(&self) -> usize {
        self.max_candidates
    }

    /// Use another character model, or none (`eval --charlm-model`, `--no-charlm`).
    pub fn set_charlm(&mut self, charlm: Option<Rerank>) {
        self.charlm = charlm.map(Rc::new);
        self.decoder.merge = self.charlm.is_none();
    }

    /// How many results the re-ranker chooses from (5 without one, as in engine.py).
    pub fn charlm_top(&self) -> usize {
        self.charlm.as_ref().map_or(5, |c| c.top)
    }

    /// Refresh a decoder's model scores after its buffer changed (no-op without models).
    pub fn rescore(&self, decoder: &mut Decoder, context: &str) {
        if let Some(c) = &self.charlm {
            c.rescore(decoder, context);
        }
    }

    /// Rebuild the dictionary's user layer after the user's dictionary changed.
    pub fn refresh_lexicon(&self) {
        let ud = self.userdict.as_ref().map(RefCell::borrow);
        self.lexicon.borrow_mut().refresh(ud.as_deref());
    }

    /// Write the user model now (on `close` and when the engine stops).
    pub fn save_user(&self) {
        if let Some(user) = &self.user {
            user.borrow_mut().save(true);
        }
    }

    pub fn set_style(&mut self, style: Style) {
        self.decoder.set_style(style);
        self.style = style;
    }

    /// A decoder with its own buffer, sharing this engine's dictionary and models.
    pub fn new_decoder(&self) -> Decoder {
        let mut decoder = Decoder::new(
            Rc::clone(&self.lexicon),
            Rc::clone(&self.lm),
            self.beam,
            self.max_candidates,
            extra_score(&self.user),
            self.word_penalty,
            self.eos_weight,
            self.style,
            Rc::clone(&self.word_bias),
        );
        decoder.merge = self.charlm.is_none();
        decoder
    }

    /// Begin a composition; `context` is the text before the cursor (never modified).
    pub fn start(&mut self, context: &str) {
        let state = context_state(context, &self.lexicon.borrow(), &self.lm);
        self.decoder.reset(Some(state));
    }

    pub fn convert(&mut self, syllables: &[String], context: &str, nbest: usize) -> Vec<(String, f64)> {
        self.start(context);
        for s in syllables {
            self.decoder.push(s.clone());
        }
        if let Some(c) = self.charlm.clone() {
            c.rescore(&mut self.decoder, context);
        }
        let top = self.charlm_top();
        let decoder = &self.decoder;
        let mut out: Vec<(String, f64)> = Vec::new();
        for (words, score) in decoder.nbest(nbest.max(top)) {
            let text: String = decoder.show(&words).iter().map(|w| &**w).collect();
            // Casual 地/得 -> 的 can make two results equal.
            if !out.iter().any(|(t, _)| *t == text) {
                out.push((text, score));
            }
        }
        if let Some(choice) = &decoder.choice {
            let pick: String = decoder.show(choice).iter().map(|w| &**w).collect();
            out.sort_by_key(|(t, _)| *t != pick);
        }
        out.truncate(nbest);
        out
    }
}
