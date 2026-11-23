//! Front-end timing log, for finding out where a slow key spends its time.
//!
//! Off unless the settings window turned it on, and then only until the time
//! written in `%LOCALAPPDATA%\Bopogram\timing-until.txt` (24 hours later).
//! Each line holds the time, the application, the kind of request and how
//! long each step took. Never anything typed: no keys, no text.

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::Win32::System::SystemInformation::GetLocalTime;

/// How often the switch file is looked at.
const RECHECK: Duration = Duration::from_secs(10);

pub fn folder() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Bopogram"))
}

thread_local! {
    /// (when last checked, on) for this thread.
    static SWITCH: Cell<Option<(Instant, bool)>> = const { Cell::new(None) };
    static APPLICATION: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Is the log on? Cheap: the file is read at most every few seconds.
pub fn on() -> bool {
    SWITCH.with(|switch| match switch.get() {
        Some((at, on)) if at.elapsed() < RECHECK => on,
        _ => {
            let on = folder()
                .and_then(|f| std::fs::read_to_string(f.join("timing-until.txt")).ok())
                .and_then(|text| text.trim().parse::<u64>().ok())
                .is_some_and(|until| now_secs() < until);
            switch.set(Some((Instant::now(), on)));
            on
        }
    })
}

/// File name of this process's program, e.g. `notepad.exe`.
fn application() -> String {
    APPLICATION.with(|name| {
        name.borrow_mut()
            .get_or_insert_with(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "?".to_owned())
            })
            .clone()
    })
}

/// The steps of one key, timed. Created for every key; costs nothing when
/// the log is off.
pub struct Key {
    on: bool,
    started: Instant,
    last: Instant,
    kind: &'static str,
    steps: Vec<(&'static str, f64)>,
}

impl Key {
    pub fn start(kind: &'static str) -> Self {
        let now = Instant::now();
        Key { on: on(), started: now, last: now, kind, steps: Vec::new() }
    }

    /// The step that just ended, measured from the previous one.
    pub fn step(&mut self, name: &'static str) {
        if self.on {
            let now = Instant::now();
            self.steps.push((name, (now - self.last).as_secs_f64() * 1000.0));
            self.last = now;
        }
    }

    /// The request(s) this key made, e.g. `key` or `commit` (operation names only).
    pub fn request(&mut self, op: Option<&'static str>) {
        if let Some(op) = op {
            self.kind = op;
        }
    }

    /// Write the line. Keys that did nothing (no step recorded) are skipped.
    pub fn finish(self) {
        if !self.on || self.steps.is_empty() {
            return;
        }
        let total = self.started.elapsed().as_secs_f64() * 1000.0;
        let mut line = format!("{}\ttotal={total:.1}", head(self.kind));
        for (name, ms) in &self.steps {
            line.push_str(&format!("\t{name}={ms:.1}"));
        }
        append(line);
    }
}

/// The start of a line: the time, the application and the kind.
fn head(kind: &str) -> String {
    let time = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}\t{}\t{kind}",
        time.wYear,
        time.wMonth,
        time.wDay,
        time.wHour,
        time.wMinute,
        time.wSecond,
        time.wMilliseconds,
        application(),
    )
}

/// A line of numbers (screen positions, say) instead of durations.
pub fn note(kind: &str, values: &[(&str, i32)]) {
    if !on() {
        return;
    }
    let mut line = head(kind);
    for (name, value) in values {
        line.push_str(&format!("\t{name}={value}"));
    }
    append(line);
}

fn append(mut line: String) {
    line.push('\n');
    if let Some(folder) = folder() {
        // One write per line, appended: lines from several applications
        // do not mix.
        let file = std::fs::OpenOptions::new().create(true).append(true).open(folder.join("timing.log"));
        if let Ok(mut file) = file {
            let _ = file.write_all(line.as_bytes());
        }
    }
}
