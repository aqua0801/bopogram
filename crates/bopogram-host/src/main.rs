//! bopogram-host: one process per user. Starts the engine, creates the
//! per-user named pipe, and relays requests from IME instances to the engine.
//!
//! Local only: the pipe rejects remote clients and there is no networking.
//!
//! Usage:
//!   bopogram-host [--python <python.exe>] [--config <runtime config.json>]
//!   bopogram-host --client
//!
//! Paths not given on the command line are read from `bopogram-host.json`
//! next to the executable: {"python": "...", "config": "..."}. An optional
//! "trace" there (or --trace <file>) makes the engine log every request and
//! response to that file, for debugging; it records everything typed.

mod client;
mod engine;
mod log;
mod server;
mod win;

use std::path::PathBuf;
use std::process::ExitCode;

use engine::EngineConfig;
use log::log;

const USAGE: &str = "usage: bopogram-host [--python <python.exe>] [--config <config.json>]\n       bopogram-host --client";

struct Args {
    client: bool,
    python: Option<PathBuf>,
    config: Option<PathBuf>,
    trace: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { client: false, python: None, config: None, trace: None };
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--client") => args.client = true,
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
    let (mut python, mut config, mut trace) = (args.python, args.config, args.trace);
    if python.is_none() || config.is_none() {
        let path = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name("bopogram-host.json");
        let text = std::fs::read_to_string(&path).map_err(|e| {
            format!("--python/--config not given and {} is unreadable: {e}", path.display())
        })?;
        let file: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let field = |name: &str| file.get(name).and_then(|v| v.as_str()).map(PathBuf::from);
        python = python.or_else(|| field("python"));
        config = config.or_else(|| field("config"));
        trace = trace.or_else(|| field("trace"));
    }
    Ok(EngineConfig {
        python: python.ok_or("no python path configured")?,
        config: config.ok_or("no engine config path configured")?,
        trace,
    })
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
    // Take the pipe name first so a second owner is detected before the
    // models are loaded; clients are only accepted once the engine is ready.
    let listener = win::PipeListener::bind(&sid).map_err(|e| format!("pipe: {e}"))?;
    let router = engine::start(cfg).map_err(|e| format!("engine: {e}"))?;
    log!("engine ready; serving");
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
