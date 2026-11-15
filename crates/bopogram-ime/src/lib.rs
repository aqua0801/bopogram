//! Bopogram TSF text service.
//!
//! This DLL is loaded into every application that uses the input method, so
//! it stays small: it turns key presses into requests for the engine (which
//! lives in `bopogram-host`, reached over a local named pipe) and shows what
//! comes back. No models, no heavy work, no networking.
//!
//! - `protocol`: the line-based JSON requests and responses.
//! - `controller`: which key does what. Pure logic, no Windows calls.
//! - `pipe`: the named pipe client, with timeouts.

pub mod controller;
pub mod pipe;
pub mod protocol;
