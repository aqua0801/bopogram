//! Optional local record of what was typed, for improving the engine
//! (`prototype/bopogram/typing_log.py`; the record format is defined there).
//!
//! Off unless the config names a file ("typing_log"). Each committed
//! composition appends one JSON line. Safeguards:
//!   * Nothing is written while learning is off (password fields), and nothing
//!     is ever sent anywhere: this only appends to one local file.
//!   * Letters and digits are never stored: each is replaced by "＊" in the
//!     items, the texts and the context.
//!   * The file stops growing at MAX_BYTES.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use serde_json::json;

pub const MAX_BYTES: u64 = 20_000_000;
const MASK: char = '＊';
const CONTEXT_CHARS: usize = 40;

/// Replace every letter and digit (ASCII and full-width) by MASK.
pub fn scrub(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '0'..='9' | 'A'..='Z' | 'a'..='z' | '０'..='９' | 'Ａ'..='Ｚ' | 'ａ'..='ｚ' => MASK,
            other => other,
        })
        .collect()
}

/// Local time as "YYYY-MM-DDTHH:MM".
fn now() -> String {
    clock::local_minute()
}

pub struct TypingLog {
    path: PathBuf,
}

/// One committed composition.
pub struct Record<'a> {
    pub items: &'a [String],
    pub first: &'a str,
    pub text: &'a str,
    pub picked: &'a [(usize, usize, String)],
    pub context: &'a str,
    pub style: &'a str,
    pub auto: bool,
}

impl TypingLog {
    pub fn new(path: PathBuf) -> TypingLog {
        TypingLog { path }
    }

    /// Append one record. False when nothing was written; a failure never
    /// interrupts typing.
    pub fn write(&self, r: &Record) -> bool {
        if r.items.iter().all(|item| item.starts_with('=')) {
            return false; // no bopomofo at all: nothing to learn from
        }
        let n = r.context.chars().count();
        let context: String = r.context.chars().skip(n.saturating_sub(CONTEXT_CHARS)).collect();
        let record = json!({
            "v": 1,
            "t": now(),
            "items": r.items.iter().map(|i| scrub(i)).collect::<Vec<_>>(),
            "first": scrub(r.first),
            "text": scrub(r.text),
            "picked": r.picked.iter().map(|(a, n, w)| json!([a, n, scrub(w)])).collect::<Vec<_>>(),
            "context": scrub(&context),
            "style": r.style,
            "auto": r.auto,
        });
        let result = (|| -> std::io::Result<bool> {
            if fs::metadata(&self.path).is_ok_and(|m| m.len() >= MAX_BYTES) {
                return Ok(false);
            }
            if let Some(dir) = self.path.parent() {
                fs::create_dir_all(dir)?;
            }
            let mut fh = fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
            writeln!(fh, "{record}")?;
            Ok(true)
        })();
        result.unwrap_or(false)
    }
}

/// The local wall clock. The standard library has no time zones, so on
/// Windows this asks the system (kernel32 only, no other component);
/// elsewhere it gives UTC.
mod clock {
    #[cfg(windows)]
    pub fn local_minute() -> String {
        #[repr(C)]
        #[derive(Default)]
        struct SystemTime {
            year: u16,
            month: u16,
            day_of_week: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            milliseconds: u16,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetLocalTime(time: *mut SystemTime);
        }
        let mut t = SystemTime::default();
        // SAFETY: GetLocalTime fills the struct it is given and has no other effect.
        unsafe { GetLocalTime(&mut t) };
        format!("{:04}-{:02}-{:02}T{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute)
    }

    #[cfg(not(windows))]
    pub fn local_minute() -> String {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
        // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + (month <= 2) as i64;
        format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}", rest / 3600, rest / 60 % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_letters_and_digits() {
        assert_eq!(scrub("ab我12ｚ！"), "＊＊我＊＊＊！");
        assert_eq!(now().len(), 16);
    }
}
