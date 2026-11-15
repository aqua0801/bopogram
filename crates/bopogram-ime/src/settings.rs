//! The one setting the front end keeps: the casual/formal style, so that a
//! choice made in one application holds in the others and after a restart.
//! A one-word file in `%LOCALAPPDATA%\Bopogram`; local only, like everything
//! else. Processes that cannot reach the folder just use the engine's default.

use std::path::PathBuf;

use crate::controller::{CASUAL, FORMAL};

fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Bopogram").join("style.txt"))
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
