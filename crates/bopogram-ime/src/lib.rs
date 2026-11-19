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
//! - `candidates`: the candidate list for applications that draw it themselves (UI-less mode).
//! - `icon`: the mode icon and the input method's icon, drawn with GDI.
//! - `langbar`: the mode button in the taskbar's input indicator, and its menu.
//! - `settings`: the style file shared by all applications.
//! - `timing`: the optional timing log (durations only, never text).

pub mod controller;
pub mod pipe;
pub use bopogram_protocol as protocol;

mod candidates;
mod display;
mod dll;
mod edit;
mod icon;
mod langbar;
mod popup;
mod register;
mod service;
mod settings;
mod timing;

pub use dll::{CLSID_TEXT_SERVICE, GUID_DISPLAY_ATTRIBUTE};

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Panics in a row (reset by any call that ends normally).
static PANICS: AtomicU32 = AtomicU32::new(0);
/// Set for the rest of the process once `PANICS` reaches `PANIC_LIMIT`.
static BROKEN: AtomicBool = AtomicBool::new(false);
const PANIC_LIMIT: u32 = 3;

/// Run `f`; a panic becomes `fallback` instead of crossing into the
/// application that loaded us. After a few in a row the input method
/// stands aside in this process (see `broken`).
fn guard<R>(fallback: R, f: impl FnOnce() -> R) -> R {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => {
            if PANICS.load(Ordering::Relaxed) != 0 {
                PANICS.store(0, Ordering::Relaxed);
            }
            result
        }
        Err(_) => {
            if PANICS.fetch_add(1, Ordering::Relaxed) + 1 >= PANIC_LIMIT {
                BROKEN.store(true, Ordering::Relaxed);
            }
            fallback
        }
    }
}

/// Something keeps failing here: every key goes to the application as if
/// no input method were selected, until the application restarts.
fn broken() -> bool {
    BROKEN.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    #[test]
    fn repeated_panics_turn_the_input_method_off() {
        let quiet = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        assert_eq!(super::guard(1, || panic!("once")), 1);
        assert_eq!(super::guard(2, || 3), 3);
        assert!(!super::broken(), "not in a row");
        for _ in 0..super::PANIC_LIMIT {
            super::guard((), || panic!("again"));
        }
        std::panic::set_hook(quiet);
        assert!(super::broken());
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
