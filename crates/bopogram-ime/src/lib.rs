//! Bopogram TSF text service.
//!
//! This DLL is loaded into every application that uses the input method, so
//! it stays small: it turns key presses into requests for the engine (which
//! lives in `bopogram-host`, reached over a local named pipe) and shows what
//! comes back. No models, no heavy work, no networking.
//!
//! Logic, testable without registering anything:
//! - `protocol`: the line-based JSON requests and responses (the
//!   `bopogram-protocol` crate, shared with the host).
//! - `controller`: which key does what. No Windows calls.
//! - `pipe`: the named pipe client, with timeouts.
//!
//! The text service itself:
//! - `dll`: DLL entry points, class factory, identifiers.
//! - `register`: what `regsvr32` runs. Never called from anywhere else.
//! - `service`: the TSF text input processor and its key event sink.
//! - `edit`: edit sessions and the operations on the document.
//! - `display`: the display attribute that underlines the composition.
//! - `popup`: the candidate window.
//! - `langbar`: the mode button in the taskbar's input indicator, and its menu.
//! - `settings`: the style file shared by all applications.
//! - `timing`: the optional timing log (durations only, never text).

pub mod controller;
pub mod pipe;
pub use bopogram_protocol as protocol;

mod display;
mod dll;
mod edit;
mod langbar;
mod popup;
mod register;
mod service;
mod settings;
mod timing;

pub use dll::{CLSID_TEXT_SERVICE, GUID_DISPLAY_ATTRIBUTE};

/// Run `f`; a panic becomes `fallback` instead of crossing into the
/// application that loaded us.
fn guard<R>(fallback: R, f: impl FnOnce() -> R) -> R {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(fallback)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
