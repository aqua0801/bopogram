//! Bopogram engine: context-aware bopomofo conversion.
//!
//! A port of the Python prototype (`prototype/bopogram/`), which stays the
//! reference: the two must give the same results for the same requests
//! (`prototype/docs/golden.md`). No Windows components, no network.
//!
//! - `lm`: the n-gram models (own file format, built from ARPA) and their mixture.
//! - `lexicon`: the pronunciation dictionary.
//! - `decoder`: the lattice and its decoding.
//! - `context`: the text before the cursor, read into the model state.
//! - `layout`: keystrokes -> syllables.
//! - `engine`: the config and one-call conversion.
//! - `session`, `symbols`: one composition with its editing rules.
//! - `server`: the `serve` protocol for the front end.

pub mod context;
pub mod decoder;
pub mod engine;
pub mod layout;
pub mod lexicon;
pub mod lm;
pub mod server;
pub mod session;
pub mod symbols;
