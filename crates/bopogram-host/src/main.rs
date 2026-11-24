//! bopogram-host: one process per user. Starts the engine, creates the
//! per-user named pipe, and relays requests from IME instances to the engine.
//!
//! Local only: the pipe rejects remote clients and there is no networking.
//!
//! Usage:
//!   bopogram-host [--engine <bopogram.exe> [--models <dir>] | --python <python.exe>]
//!                 [--config <runtime config.json>]
//!   bopogram-host --client
//!
//! Paths not given on the command line are read from `bopogram-host.json`
//! next to the executable: {"engine": "...", "models": "...", "python": "...",
//! "config": "..."}. With "engine" the Rust engine runs (its model files in
//! "models" if they are not beside the config's); without it, the Python
//! prototype. "python" is also what the settings window runs tools with. An optional
//! "trace" there (or --trace <file>) makes the engine log every request and
//! response to that file, for debugging; it records everything typed.
//!
//! A GUI-subsystem program, so that starting it (at sign-in, after install)
//! opens no console window. `--client` therefore reads requests from a pipe
//! or file, not from a console: `Get-Content requests.txt | bopogram-host --client`.
#![windows_subsystem = "windows"]

mod client;
mod engine;
mod log;
mod server;
mod win;

use std::path::PathBuf;
use std::process::ExitCode;

use engine::{EngineConfig, EngineKind};
use log::log;

const USAGE: &str = "usage: bopogram-host [--engine <bopogram.exe> [--models <dir>] | --python <python.exe>] [--config <config.json>]\n       bopogram-host --client";

struct Args {
    client: bool,
    engine: Option<PathBuf>,
    models: Option<PathBuf>,
    python: Option<PathBuf>,
    config: Option<PathBuf>,
    trace: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { client: false, engine: None, models: None, python: None, config: None, trace: None };
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--client") => args.client = true,
            Some("--engine") => args.engine = Some(it.next().ok_or("--engine needs a path")?.into()),
            Some("--models") => args.models = Some(it.next().ok_or("--models needs a path")?.into()),
            Some("--python") => {
                args.python = Some(it.next().ok_or("--python needs a path")?.into())
            }
            Some("--config") => {
                args.config = Some(it.next().ok_or("--config needs a path")?.into())
            }
            Some("--trace") => args.trace = Some(it.next().ok_or("--trace needs a path")?.into()),
            _ => return Err(format!("unknown argument {arg:?}")),
        }
    }
    Ok(args)
}

/// Fill in whatever the command line left out from `bopogram-host.json`.
fn engine_config(args: Args) -> Result<EngineConfig, String> {
    let (mut engine, mut models) = (args.engine, args.models);
    let (mut python, mut config, mut trace) = (args.python, args.config, args.trace);
    if config.is_none() || (engine.is_none() && python.is_none()) {
        let path = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name("bopogram-host.json");
        let text = std::fs::read_to_string(&path).map_err(|e| {
            format!("--python/--config not given and {} is unreadable: {e}", path.display())
        })?;
        let file: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let field = |name: &str| file.get(name).and_then(|v| v.as_str()).map(PathBuf::from);
        // A Python given on the command line means the Python engine.
        if python.is_none() {
            engine = engine.or_else(|| field("engine"));
            models = models.or_else(|| field("models"));
        }
        python = python.or_else(|| field("python"));
        config = config.or_else(|| field("config"));
        trace = trace.or_else(|| field("trace"));
    }
    let kind = match (engine, python) {
        (Some(exe), _) => EngineKind::Native { exe, models },
        (None, Some(python)) => EngineKind::Python(python),
        (None, None) => return Err("no engine configured (\"engine\" or \"python\")".into()),
    };
    Ok(EngineConfig { kind, config: config.ok_or("no engine config path configured")?, trace })
}

fn run() -> Result<(), String> {
    let args = parse_args().map_err(|e| format!("{e}\n{USAGE}"))?;
    let sid = win::user_sid().map_err(|e| format!("cannot read the user SID: {e}"))?;
    if args.client {
        return client::run(&win::pipe_name(&sid)).map_err(|e| format!("client: {e}"));
    }

    let cfg = engine_config(args)?;
    let Some(_mutex) = win::single_instance(&sid).map_err(|e| format!("mutex: {e}"))? else {
        // Another host is already serving this user.
        return Ok(());
    };
    log::init();
    log!("host starting");
    // Take the pipe name first, then serve at once: while the engine loads,
    // requests are answered with "starting" and the input method keeps them.
    let listener = win::PipeListener::bind(&sid).map_err(|e| format!("pipe: {e}"))?;
    let router = engine::start(cfg);
    log!("serving; engine loading");
    server::run(listener, router)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            log!("{message}");
            ExitCode::FAILURE
        }
    }
}
