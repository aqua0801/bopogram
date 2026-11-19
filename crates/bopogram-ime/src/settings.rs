//! The settings the front end keeps itself, in `%LOCALAPPDATA%\Bopogram`
//! (local only, like everything else), shared by all applications:
//!
//! - `style.txt`: casual or formal, so that a choice made in one application
//!   holds in the others and after a restart.
//! - `candidate-font.txt`: the text size of the candidate window, in pixels
//!   at 96 DPI (written by the settings window).
//!
//! Processes that cannot reach the folder use the defaults.

use std::cell::Cell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::controller::{CASUAL, FORMAL};

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
