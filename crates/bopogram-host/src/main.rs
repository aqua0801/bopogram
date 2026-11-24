//! bopogram-host: one process per user. Runs the engine, creates the
//! per-user named pipe, and relays requests from IME instances to the engine.
//!
//! Local only: the pipe rejects remote clients and there is no networking.
//!
//! Usage:
//!   bopogram-host [--models <dir> | --python <python.exe>] [--config <runtime config.json>]
//!   bopogram-host --client
//!
//! Settings not given on the command line are read from `bopogram-host.json`
//! next to the executable: {"engine": "rust" | "python", "models": "...",
//! "python": "...", "config": "..."}. The Rust engine runs inside this process
//! (its model files in "models" if they are not beside the config's); with
//! "engine": "python" (or --python) the Python prototype runs instead, as a
//! child process. "python" is also what the settings window runs tools with.
//! An optional "trace" there (or --trace <file>) makes the engine log every
//! request and response to that file, for debugging; it records everything typed.
//!
//! To stop, set the event `Local\bopogram-host-stop-<user SID>` (the install
//! scripts do): the engine saves the personal model and the host exits.
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

const USAGE: &str = "usage: bopogram-host [--models <dir> | --python <python.exe>] [--config <config.json>]\n       bopogram-host --client";

struct Args {
    client: bool,
    models: Option<PathBuf>,
    python: Option<PathBuf>,
    config: Option<PathBuf>,
    trace: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { client: false, models: None, python: None, config: None, trace: None };
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--client") => args.client = true,
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
    // A Python given on the command line means the Python engine.
    let mut use_python = args.python.is_some();
    let (mut models, mut python) = (args.models, args.python);
    let (mut config, mut trace) = (args.config, args.trace);
    let path = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name("bopogram-host.json");
    // The file is needed only for what the command line leaves out.
    let read = std::fs::read_to_string(&path);
    if read.is_err() && config.is_some() {
        // Everything else has a default.
    } else {
        let text = read.map_err(|e| {
            format!("--config not given and {} is unreadable: {e}", path.display())
        })?;
        let file: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let field = |name: &str| file.get(name).and_then(|v| v.as_str()).map(PathBuf::from);
        if !use_python {
            // "engine": "python" picks the prototype; anything else (or nothing) the Rust engine.
            use_python = file.get("engine").and_then(|v| v.as_str()) == Some("python");
            models = models.or_else(|| field("models"));
        }
        python = python.or_else(|| field("python"));
        config = config.or_else(|| field("config"));
        trace = trace.or_else(|| field("trace"));
    }
    let kind = if use_python {
        EngineKind::Python(python.ok_or("\"engine\" is \"python\" but no \"python\" is configured")?)
    } else {
        EngineKind::Builtin { models }
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
    if let Some(file) = log::handle() {
        win::set_stderr(file);
    }
    log!("host starting");
    // Take the pipe name first, then serve at once: while the engine loads,
    // requests are answered with "starting" and the input method keeps them.
    let listener = win::PipeListener::bind(&sid).map_err(|e| format!("pipe: {e}"))?;
    let router = engine::start(cfg);
    let stopping = std::sync::Arc::clone(&router);
    win::on_stop_event(&sid, move || {
        log!("stop requested");
        stopping.stop();
    })
    .map_err(|e| format!("stop event: {e}"))?;
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
