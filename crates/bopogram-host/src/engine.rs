//! Runs the engine and routes lines between it and the pipe connections.
//!
//! The engine speaks one JSON object per line. Every request forwarded to it
//! carries the `sid` of the connection it came from; the engine echoes `sid`
//! back, which is how a response finds its connection.
//!
//! The Rust engine runs inside this process, on a thread of its own that
//! owns it (its data is not shared between threads): requests reach it
//! through a channel. The Python prototype, kept to compare with, runs as a
//! child process on stdin/stdout.
//!
//! The pipe is served from the start; the engine loads in the background and
//! requests that arrive meanwhile are answered with "starting", which the
//! input method keeps and sends again.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::Value;

use crate::log::log;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Which engine runs: both speak the same protocol and take the same config.
pub enum EngineKind {
    /// The Rust engine, in this process, with the folder of its model files
    /// if they are not beside the config's models.
    Builtin { models: Option<PathBuf> },
    /// The Python prototype: an interpreter that has the `bopogram` package.
    Python(PathBuf),
}

pub struct EngineConfig {
    pub kind: EngineKind,
    /// `config.json` of the runtime bundle.
    pub config: PathBuf,
    /// Debugging: the engine appends every request and response to this
    /// file. It records everything typed, so it is only ever set by hand.
    pub trace: Option<PathBuf>,
}

/// Where request lines go.
enum Sink {
    Thread(Sender<String>),
    Child(ChildStdin),
}

pub struct Router {
    /// `None` while the engine is loading or restarting, and once stopping.
    sink: Mutex<Option<Sink>>,
    connections: Mutex<HashMap<u64, Sender<String>>>,
    next_sid: AtomicU64,
    stopping: AtomicBool,
}

impl Router {
    /// Allocate a session id for a new connection.
    pub fn register(&self) -> (u64, Receiver<String>) {
        let sid = self.next_sid.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        self.connections.lock().unwrap().insert(sid, tx);
        (sid, rx)
    }

    /// Drop a connection and tell the engine to forget its session.
    pub fn unregister(&self, sid: u64) {
        self.connections.lock().unwrap().remove(&sid);
        self.send(&format!("{{\"op\":\"close\",\"sid\":{sid}}}"));
    }

    /// Send one request line to the engine. False if the engine is not
    /// running (loading, restarting, or stopping).
    pub fn send(&self, line: &str) -> bool {
        let mut guard = self.sink.lock().unwrap();
        let sent = match guard.as_mut() {
            None => return false,
            Some(Sink::Thread(tx)) => tx.send(line.to_owned()).is_ok(),
            Some(Sink::Child(stdin)) => stdin
                .write_all(line.as_bytes())
                .and_then(|_| stdin.write_all(b"\n"))
                .and_then(|_| stdin.flush())
                .is_ok(),
        };
        if !sent {
            *guard = None;
        }
        sent
    }

    /// Stop taking requests and let the engine finish: it saves the personal
    /// model, and then the host exits (see `start`).
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        // Closing the channel (or the child's stdin) is the engine's signal.
        self.sink.lock().unwrap().take();
    }

    /// Hand a response line to the connection named by its `sid`.
    /// `sid` is removed so the client sees the plain protocol.
    fn dispatch(&self, line: &str) {
        let Ok(Value::Object(mut response)) = serde_json::from_str::<Value>(line) else {
            log!("engine sent a line that is not a JSON object; dropped");
            return;
        };
        // An internal error of the engine: one line with the kind of error
        // (the text before the colon; the rest could quote what was typed).
        if response.get("ok") == Some(&Value::Bool(false)) && !response.contains_key("preedit") {
            let error = response.get("error").and_then(Value::as_str).unwrap_or("");
            log!("engine error: {}", error.split(':').next().unwrap_or(""));
        }
        let Some(sid) = response.remove("sid").and_then(|v| v.as_u64()) else {
            return;
        };
        if let Some(tx) = self.connections.lock().unwrap().get(&sid) {
            let _ = tx.send(Value::Object(response).to_string());
        }
    }
}

/// Start the engine in the background and return at once.
pub fn start(cfg: EngineConfig) -> Arc<Router> {
    let router = Arc::new(Router {
        sink: Mutex::new(None),
        connections: Mutex::new(HashMap::new()),
        next_sid: AtomicU64::new(1),
        stopping: AtomicBool::new(false),
    });
    let shared = Arc::clone(&router);
    thread::spawn(move || match cfg.kind {
        EngineKind::Builtin { ref models } => {
            let models = models.clone();
            // Anything that escapes the engine's own per-request guard ends the
            // host; the input method lets keys through until it starts a new one.
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_builtin(&cfg, models, &shared)));
            if run.is_err() {
                log!("engine thread failed; host is stopping");
                std::process::exit(1);
            }
        }
        EngineKind::Python(ref python) => run_python(&cfg, python, &shared),
    });
    router
}

// -- the Rust engine, in this process -------------------------------------------------

fn run_builtin(cfg: &EngineConfig, models: Option<PathBuf>, router: &Router) {
    use bopogram::engine::Engine;
    use bopogram::server::{Reply, Server};

    let engine = match Engine::load(&cfg.config, true, models.as_deref()) {
        Ok(engine) => Rc::new(engine),
        Err(err) => {
            log!("engine did not start: {err}; host is stopping");
            std::process::exit(1);
        }
    };
    let trace: Option<Box<dyn Write>> = match &cfg.trace {
        Some(path) => {
            log!("TRACE IS ON: everything typed is written to {}", path.display());
            match std::fs::OpenOptions::new().create(true).append(true).open(path) {
                Ok(file) => Some(Box::new(file)),
                Err(err) => {
                    log!("trace file cannot be opened: {err}");
                    None
                }
            }
        }
        None => None,
    };
    let layout = bopogram::layout::layout("dachen").expect("the default layout exists");
    let mut server = Server::new(Rc::clone(&engine), layout, trace);
    let (tx, rx) = channel::<String>();
    *router.sink.lock().unwrap() = Some(Sink::Thread(tx));
    if router.stopping.load(Ordering::SeqCst) {
        // Asked to stop while loading.
        router.sink.lock().unwrap().take();
    }
    log!("engine ready (builtin)");
    // Until the channel closes: `stop` (or the end of the process).
    for line in rx {
        if let Reply::Line(text) = server.handle_line(&line) {
            router.dispatch(&text);
        }
    }
    engine.save_user();
    log!("engine stopped; personal model saved");
    std::process::exit(0);
}

// -- the Python prototype, as a child process --------------------------------------------

struct PyEngine {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Read one line; `None` at end of stream.
fn read_line(reader: &mut impl BufRead) -> Option<String> {
    let mut buf = Vec::new();
    match reader.read_until(b'\n', &mut buf) {
        Ok(n) if n > 0 => Some(String::from_utf8_lossy(&buf).trim().to_owned()),
        _ => None,
    }
}

/// Start the Python engine and wait until it reports that the models are loaded.
fn spawn(cfg: &EngineConfig, python: &PathBuf) -> io::Result<PyEngine> {
    let stderr = crate::log::handle().map(Stdio::from).unwrap_or_else(Stdio::null);
    let mut command = Command::new(python);
    // Without -X utf8, Python uses the ANSI code page for piped stdio.
    command.args(["-X", "utf8", "-m", "bopogram.cli", "--config"]).arg(&cfg.config).arg("serve");
    if let Some(trace) = &cfg.trace {
        log!("TRACE IS ON: everything typed is written to {}", trace.display());
        command.arg("--trace").arg(trace);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    let stdin = child.stdin.take().expect("stdin is piped");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    loop {
        let Some(line) = read_line(&mut stdout) else {
            let status = child.wait()?;
            return Err(io::Error::other(format!("engine exited before it was ready ({status})")));
        };
        let ready = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|v| v.get("ready").and_then(Value::as_bool))
            .unwrap_or(false);
        if ready {
            return Ok(PyEngine { child, stdin, stdout });
        }
    }
}

/// If the engine stops it is restarted once; failing to start, or stopping
/// again, ends the host.
fn run_python(cfg: &EngineConfig, python: &PathBuf, router: &Router) {
    let mut restarted = false;
    loop {
        let engine = match spawn(cfg, python) {
            Ok(engine) => engine,
            Err(err) => {
                log!("engine did not start: {err}; host is stopping");
                std::process::exit(1);
            }
        };
        log!("{} (python)", if restarted { "engine restarted" } else { "engine ready" });
        let PyEngine { mut child, stdin, mut stdout } = engine;
        *router.sink.lock().unwrap() = Some(Sink::Child(stdin));
        if router.stopping.load(Ordering::SeqCst) {
            router.sink.lock().unwrap().take();
        }
        while let Some(line) = read_line(&mut stdout) {
            if !line.is_empty() {
                router.dispatch(&line);
            }
        }
        router.sink.lock().unwrap().take();
        match child.wait() {
            Ok(status) => log!("engine exited ({status})"),
            Err(err) => log!("engine exited (wait failed: {err})"),
        }
        if router.stopping.load(Ordering::SeqCst) {
            log!("engine stopped");
            std::process::exit(0);
        }
        if restarted {
            log!("engine exited again after a restart; host is stopping");
            std::process::exit(1);
        }
        restarted = true;
    }
}
