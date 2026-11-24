//! Engine: configuration, loading, and one-call conversion
//! (`prototype/bopogram/engine.py`).
//!
//! The config is the runtime bundle's `config.json`, shared with the Python
//! engine. Model paths there name KenLM files (`*.binary`); this engine reads
//! its own format, from the file of the same name with the extension `.bglm`
//! (built by `bopogram build-lm`). A path that already ends in `.bglm` is used as is.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::Value;

use crate::context::context_state;
use crate::decoder::{Decoder, ExtraScore, Style};
use crate::lexicon::Lexicon;
use crate::lm::MixtureLM;
use crate::typing_log::TypingLog;
use crate::user::UserModel;
use crate::userdict::UserDict;

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

/// Our model file for a model path in the config.
pub fn model_file(path: &Path) -> PathBuf {
    if path.extension().is_some_and(|e| e == "bglm") {
        path.to_path_buf()
    } else {
        path.with_extension("bglm")
    }
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
    pub charlm_top: usize,
}

impl Engine {
    /// `use_user_model` false leaves out everything personal (user model, user
    /// dictionary, typing log), as one-off conversions do.
    pub fn load(config_file: &Path, use_user_model: bool) -> io::Result<Engine> {
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
            let file = model_file(&base.join(path));
            if !file.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{}: missing; build it from the model's ARPA file with `bopogram build-lm`", file.display()),
                ));
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
        let decoder = Decoder::new(
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
        if cfg.get("charlm").is_some_and(|c| !c.is_null()) {
            eprintln!("warning: \"charlm\" in the config is not supported yet and is ignored");
        }
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
            charlm_top: 5,
        })
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
        Decoder::new(
            Rc::clone(&self.lexicon),
            Rc::clone(&self.lm),
            self.beam,
            self.max_candidates,
            extra_score(&self.user),
            self.word_penalty,
            self.eos_weight,
            self.style,
            Rc::clone(&self.word_bias),
        )
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
        let decoder = &self.decoder;
        let mut out: Vec<(String, f64)> = Vec::new();
        for (words, score) in decoder.nbest(nbest.max(self.charlm_top)) {
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
