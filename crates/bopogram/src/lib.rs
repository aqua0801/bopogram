//! Bopogram engine: context-aware bopomofo conversion.
//!
//! A port of the Python prototype (`prototype/bopogram/`), which stays the
//! reference: the two must give the same results for the same requests
//! (`prototype/docs/golden.md`). No Windows components, no network.
//!
//! - `lm`: the n-gram models (own file format, built from ARPA) and their mixture.
//! - `lexicon`: the pronunciation dictionary.
//! - `decoder`: the lattice and its decoding.
//! - `charlm`: the character model that re-ranks the decoder's best results.
//! - `context`: the text before the cursor, read into the model state.
//! - `layout`: keystrokes -> syllables.
//! - `engine`: the config and one-call conversion.
//! - `session`, `symbols`: one composition with its editing rules.
//! - `server`: the `serve` protocol for the front end.
//! - `userdict`, `dictops`: the user's own words and exclusions.
//! - `user`: personal learning; `typing_log`: the optional local record.

pub mod charlm;
pub mod context;
pub mod decoder;
pub mod dictops;
pub mod engine;
pub mod layout;
pub mod lexicon;
pub mod lm;
pub mod server;
pub mod session;
pub mod symbols;
pub mod typing_log;
pub mod user;
pub mod userdict;
