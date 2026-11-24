//! Minimal append-only log at `%LOCALAPPDATA%\Bopogram\host.log`.
//!
//! Only lifecycle events and errors are logged. Request and response lines
//! (what the user types) must never be written here.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_BYTES: u64 = 1024 * 1024;

static LOG: Mutex<Option<File>> = Mutex::new(None);

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("Bopogram").join("host.log"))
}

/// Open the log file, starting over when it has grown past the size limit.
pub fn init() {
    let Some(path) = path() else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if fs::metadata(&path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = fs::remove_file(&path);
    }
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        *LOG.lock().unwrap() = Some(file);
    }
}

/// A second handle to the log file, for the engine's stderr (the Python
/// engine's, and this process's own for the built-in engine).
pub fn handle() -> Option<File> {
    LOG.lock().unwrap().as_ref().and_then(|f| f.try_clone().ok())
}

pub fn write(message: &str) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{secs}] {message}\n");
    // Once the log is open, standard error goes to it too (the engine's
    // warnings): write the line once.
    match LOG.lock().unwrap().as_mut() {
        Some(file) => {
            let _ = file.write_all(line.as_bytes());
        }
        None => eprint!("{line}"),
    }
}

macro_rules! log {
    ($($arg:tt)*) => { $crate::log::write(&format!($($arg)*)) };
}
pub(crate) use log;
