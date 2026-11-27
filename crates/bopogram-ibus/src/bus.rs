//! Where IBus's D-Bus is: `$IBUS_ADDRESS` (ibus-daemon sets it for the
//! engines it starts), else the address file ibus-daemon writes under
//! `~/.config/ibus/bus/` (the newest one whose daemon is alive), else what
//! `ibus address` says.

use std::path::PathBuf;

pub fn address() -> Option<String> {
    if let Ok(address) = std::env::var("IBUS_ADDRESS") {
        if !address.is_empty() {
            return Some(address);
        }
    }
    from_files().or_else(from_command)
}

fn from_files() -> Option<String> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::home().join(".config"));
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(config.join("ibus/bus"))
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    files.sort();
    files.iter().rev().find_map(|(_, path)| {
        let text = std::fs::read_to_string(path).ok()?;
        let field = |name: &str| text.lines().find_map(|line| line.strip_prefix(name)).map(str::trim);
        let address = field("IBUS_ADDRESS=")?;
        let alive = field("IBUS_DAEMON_PID=").is_none_or(|pid| PathBuf::from(format!("/proc/{pid}")).exists());
        (alive && !address.is_empty()).then(|| address.to_owned())
    })
}

fn from_command() -> Option<String> {
    let out = std::process::Command::new("ibus").arg("address").output().ok()?;
    let address = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !address.is_empty() && !address.contains("(null)")).then_some(address)
}
