//! The settings the front end keeps itself, in `%LOCALAPPDATA%\Bopogram`
//! (local only, like everything else), shared by all applications:
//!
//! - `style.txt`: casual or formal, so that a choice made in one application
//!   holds in the others and after a restart.
//! - `candidate-font.txt`: the text size of the candidate window, in pixels
//!   at 96 DPI (written by the settings window).
//! - `mode-scope.txt`: `global` if the Chinese/English mode is shared by all
//!   applications (written by the settings window); otherwise each
//!   application keeps its own, as Microsoft's input methods do.
//! - `mode.txt`: the shared mode, `chinese` or `english`.
//! - `keys.json`: selection keys, candidates per page, the mode a new
//!   application starts in, whether Shift alone switches the mode, and the
//!   phrase menu's key (written by the settings window; see `Options`).
//! - `phrases.json`: the phrase menu (written by the settings window).
//!   Read each time the menu opens.
//!
//! Processes that cannot reach the folder use the defaults.

use std::cell::Cell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::controller::{parse_phrases, Options, Phrase, CASUAL, FORMAL};

/// Candidate text size when nothing is set.
pub const DEFAULT_FONT_PX: i32 = 18;
/// How often the size file is looked at again.
const RECHECK: Duration = Duration::from_secs(5);

fn folder() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Bopogram"))
}

fn path() -> Option<PathBuf> {
    Some(folder()?.join("style.txt"))
}

pub fn load_style() -> Option<String> {
    let text = std::fs::read_to_string(path()?).ok()?;
    let style = text.trim();
    (style == CASUAL || style == FORMAL).then(|| style.to_owned())
}

pub fn save_style(style: &str) {
    if let Some(path) = path() {
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let _ = std::fs::write(path, style);
    }
}

thread_local! {
    static SHARED: Cell<Option<(Instant, bool)>> = const { Cell::new(None) };
}

/// `shared_mode`, read at most every few seconds (it is asked on every key).
pub fn shared_mode_cached() -> bool {
    SHARED.with(|cached| match cached.get() {
        Some((at, shared)) if at.elapsed() < RECHECK => shared,
        _ => {
            let shared = shared_mode();
            cached.set(Some((Instant::now(), shared)));
            shared
        }
    })
}

/// Is the Chinese/English mode shared by all applications?
pub fn shared_mode() -> bool {
    folder()
        .and_then(|f| std::fs::read_to_string(f.join("mode-scope.txt")).ok())
        .is_some_and(|text| text.trim() == "global")
}

/// The shared mode: true for Chinese. `None` if never saved.
pub fn load_mode() -> Option<bool> {
    match std::fs::read_to_string(folder()?.join("mode.txt")).ok()?.trim() {
        "chinese" => Some(true),
        "english" => Some(false),
        _ => None,
    }
}

pub fn save_mode(chinese: bool) {
    if let Some(folder) = folder() {
        let _ = std::fs::create_dir_all(&folder);
        let _ = std::fs::write(folder.join("mode.txt"), if chinese { "chinese" } else { "english" });
    }
}

thread_local! {
    static OPTIONS: Cell<Option<(Instant, Options)>> = const { Cell::new(None) };
}

/// The key options in `keys.json`, read at most every few seconds.
pub fn options() -> Options {
    OPTIONS.with(|cached| match cached.get() {
        Some((at, options)) if at.elapsed() < RECHECK => options,
        _ => {
            let options = folder()
                .and_then(|f| std::fs::read_to_string(f.join("keys.json")).ok())
                .map_or_else(Options::default, |text| Options::parse(&text));
            cached.set(Some((Instant::now(), options)));
            options
        }
    })
}

/// The phrases of the phrase menu; empty if there are none.
pub fn load_phrases() -> Vec<Phrase> {
    folder()
        .and_then(|f| std::fs::read_to_string(f.join("phrases.json")).ok())
        .map_or_else(Vec::new, |text| parse_phrases(&text))
}

thread_local! {
    static FONT: Cell<Option<(Instant, i32)>> = const { Cell::new(None) };
}

/// The candidate window's text size in pixels at 96 DPI.
pub fn candidate_font_px() -> i32 {
    FONT.with(|cached| match cached.get() {
        Some((at, px)) if at.elapsed() < RECHECK => px,
        _ => {
            let px = folder()
                .and_then(|f| std::fs::read_to_string(f.join("candidate-font.txt")).ok())
                .and_then(|text| text.trim().parse::<i32>().ok())
                .filter(|px| (12..=40).contains(px))
                .unwrap_or(DEFAULT_FONT_PX);
            cached.set(Some((Instant::now(), px)));
            px
        }
    })
}
