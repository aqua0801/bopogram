//! Owns the engine child process and routes lines between it and the
//! pipe connections.
//!
//! The engine speaks one JSON object per line on stdin/stdout. Every request
//! forwarded to it carries the `sid` of the connection it came from; the
//! engine echoes `sid` back, which is how a response finds its connection.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::Value;

use crate::log::log;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct EngineConfig {
    /// Python interpreter that has the `bopogram` package installed.
    pub python: PathBuf,
    /// `config.json` of the runtime bundle.
    pub config: PathBuf,
    /// Debugging: the engine appends every request and response to this
    /// file. It records everything typed, so it is only ever set by hand.
    pub trace: Option<PathBuf>,
}

pub struct Router {
    /// `None` while the engine is down or restarting.
    stdin: Mutex<Option<ChildStdin>>,
    connections: Mutex<HashMap<u64, Sender<String>>>,
    next_sid: AtomicU64,
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

    /// Send one request line to the engine. False if the engine is down.
    pub fn send(&self, line: &str) -> bool {
        let mut guard = self.stdin.lock().unwrap();
        let Some(stdin) = guard.as_mut() else {
            return false;
        };
        let sent = stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush());
        if sent.is_err() {
            *guard = None;
        }
        sent.is_ok()
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

struct Engine {
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

/// Start the engine and wait until it reports that the models are loaded.
fn spawn(cfg: &EngineConfig) -> io::Result<Engine> {
    let stderr = crate::log::handle().map(Stdio::from).unwrap_or_else(Stdio::null);
    let mut command = Command::new(&cfg.python);
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
            return Err(io::Error::other(format!(
                "engine exited before it was ready ({status})"
            )));
        };
        let ready = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|v| v.get("ready").and_then(Value::as_bool))
            .unwrap_or(false);
        if ready {
            return Ok(Engine { child, stdin, stdout });
        }
    }
}

/// Start the engine and the thread that reads its responses. Returns once
/// the engine is ready. If the engine later exits it is restarted once;
/// a second exit ends the host.
pub fn start(cfg: EngineConfig) -> io::Result<Arc<Router>> {
    let mut engine = spawn(&cfg)?;
    let router = Arc::new(Router {
        stdin: Mutex::new(Some(engine.stdin)),
        connections: Mutex::new(HashMap::new()),
        next_sid: AtomicU64::new(1),
    });
    let shared = Arc::clone(&router);
    thread::spawn(move || {
        let mut restarted = false;
        loop {
            while let Some(line) = read_line(&mut engine.stdout) {
                if !line.is_empty() {
                    shared.dispatch(&line);
                }
            }
            *shared.stdin.lock().unwrap() = None;
            match engine.child.wait() {
                Ok(status) => log!("engine exited ({status})"),
                Err(err) => log!("engine exited (wait failed: {err})"),
            }
            if restarted {
                log!("engine exited again after a restart; host is stopping");
                std::process::exit(1);
            }
            restarted = true;
            match spawn(&cfg) {
                Ok(fresh) => {
                    log!("engine restarted");
                    *shared.stdin.lock().unwrap() = Some(fresh.stdin);
                    engine.child = fresh.child;
                    engine.stdout = fresh.stdout;
                }
                Err(err) => {
                    log!("engine restart failed: {err}; host is stopping");
                    std::process::exit(1);
                }
            }
        }
    });
    Ok(router)
}
