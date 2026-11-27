//! Bopogram for Linux: an IBus engine.
//!
//! ibus-daemon starts this program (`--ibus`, from the component file
//! installed in `/usr/share/ibus/component/`). It connects to IBus's own
//! D-Bus, a local socket, and offers a factory that makes one engine object
//! per input context. The Rust engine runs in this process, on a thread of
//! its own; the key rules are the Windows front end's (`bopogram-controller`).
//!
//! No network: the only connection is the local IBus socket.
//!
//! Data: `$BOPOGRAM_DATA`, else `$XDG_DATA_HOME/bopogram`, else
//! `~/.local/share/bopogram`: the model pack in `models/` (`bopogram pack
//! install`, the engine's configuration is `models/engine.json`), or a
//! `config.json` and what it names (the test kit). Log, which never
//! records typed text: `~/.cache/bopogram/ibus.log`.
//!
//! `bopogram-ibus --check` runs the whole key path without IBus: it types a
//! few sentences through the controller and prints what would be committed.

mod keys;
mod log;
mod remote;

#[cfg(unix)]
mod bus;
#[cfg(unix)]
mod engine;
#[cfg(unix)]
mod values;

use std::path::PathBuf;
use std::process::ExitCode;

pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("BOPOGRAM_DATA").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("bopogram");
    }
    home().join(".local/share/bopogram")
}

/// The engine's configuration: `models/engine.json` once a model pack is
/// installed (`bopogram pack install`), else `config.json` (the test kit).
pub fn engine_config() -> PathBuf {
    let dir = data_dir();
    let pack = dir.join("models").join("engine.json");
    if pack.is_file() { pack } else { dir.join("config.json") }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// The settings of the key rules: `keys.json` in the data folder (the same
/// file as on Windows), defaults otherwise.
pub fn options() -> bopogram_controller::Options {
    let file = data_dir().join("keys.json");
    std::fs::read_to_string(file).map(|text| bopogram_controller::Options::parse(&text)).unwrap_or_default()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--check") {
        return check();
    }
    if args.iter().any(|a| a == "--version") {
        println!("bopogram-ibus {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    run()
}

#[cfg(unix)]
fn run() -> ExitCode {
    log::log("starting");
    let shared = remote::start(engine_config());
    match engine::serve(shared) {
        Ok(()) => {
            log::log("the bus went away; stopping");
            ExitCode::SUCCESS
        }
        Err(e) => {
            log::log(&format!("error: {e}"));
            eprintln!("bopogram-ibus: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(unix))]
fn run() -> ExitCode {
    eprintln!("bopogram-ibus runs on Linux only (use --check to try the key path)");
    ExitCode::FAILURE
}

/// Type sentences through the controller, as IBus would deliver the keys.
fn check() -> ExitCode {
    use bopogram_controller::Controller;
    let config = engine_config();
    println!("data: {}", config.display());
    let shared = remote::start(config);
    if !shared.wait_ready() {
        eprintln!("the engine did not load (see the log: {})", log::path().display());
        return ExitCode::FAILURE;
    }
    let mut controller = Controller::new(remote::Client::new(shared));
    controller.set_options(options());
    let mut ok = true;
    for (keys, want) in [("ji3y94d04d04", "我再看看"), ("ji3y94ru8 ", "我在家"), ("1j420 tjp6g4qup6xm4", "不單純是頻率")] {
        let mut committed = String::new();
        let mut shown = String::new();
        for c in keys.chars() {
            let keyval = c as u32;
            let outcome = controller.key_down(keys::key(keyval, 0));
            committed.extend(outcome.commit);
            shown = controller.preedit().to_owned();
            controller.key_up(keys::key(keyval, keys::RELEASE_MASK));
        }
        let outcome = controller.key_down(keys::key(keys::RETURN, 0));
        committed.extend(outcome.commit);
        let good = committed == want;
        ok &= good;
        println!("{keys:24} preedit {shown}  commit {committed}  {}", if good { "ok" } else { "DIFFERENT" });
    }
    // Shift alone switches to English: letters then reach the application.
    let shift = keys::key(0xffe1, 0);
    controller.key_down(shift);
    controller.key_up(keys::key(0xffe1, keys::SHIFT_MASK | keys::RELEASE_MASK));
    let passed = !controller.key_down(keys::key('j' as u32, 0)).eaten;
    ok &= passed && !controller.chinese();
    println!("Shift alone -> English, j passes to the application: {}", if passed { "ok" } else { "DIFFERENT" });
    if ok {
        println!("all ok");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
