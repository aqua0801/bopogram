//! A small log for finding out why the input method misbehaves. It records
//! what happened (started, connected, engine loaded, errors), never what was
//! typed. `~/.cache/bopogram/ibus.log`, started afresh once it passes 1 MB.

use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const LIMIT: u64 = 1 << 20;

pub fn path() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::home().join(".cache"));
    cache.join("bopogram").join("ibus.log")
}

pub fn log(message: &str) {
    let path = path();
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
    let fresh = std::fs::metadata(&path).is_ok_and(|m| m.len() > LIMIT);
    let file = std::fs::OpenOptions::new().create(true).append(!fresh).write(true).truncate(fresh).open(&path);
    if let Ok(mut file) = file {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(
            file,
            "{:02}:{:02}:{:02}Z [{}] {message}",
            secs / 3600 % 24,
            secs / 60 % 60,
            secs % 60,
            std::process::id()
        );
    }
}
