//! The engine, on a thread of its own (its data is not shared between
//! threads), and the clients that reach it through a channel: one per input
//! context, each with a session (`sid`) of its own in the engine.
//!
//! The engine loads in the background; until it is ready requests are
//! answered with `Failure::Starting`, which the controller keeps and sends
//! again (as with the Windows host).

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use bopogram::engine::Engine;
use bopogram::server::{Reply, Server};
use bopogram_protocol::{Failure, Op, Response};
use serde_json::Value;

use crate::log::log;

const LOADING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;

/// A request answered later than this counts as late (the controller then
/// catches up). The engine itself takes a few milliseconds.
const TIMEOUT: Duration = Duration::from_secs(1);
/// The user model is written after this long without a request.
const SAVE_AFTER: Duration = Duration::from_secs(10);

struct Job {
    line: String,
    reply: Option<Sender<String>>,
}

pub struct Shared {
    jobs: Mutex<Sender<Job>>,
    state: AtomicU8,
    loaded: (Mutex<bool>, Condvar),
}

impl Shared {
    fn state(&self) -> u8 {
        self.state.load(Ordering::Acquire)
    }

    fn send(&self, job: Job) -> bool {
        self.jobs.lock().map(|jobs| jobs.send(job).is_ok()).unwrap_or(false)
    }

    /// Block until the engine has loaded (or failed to). True if it is ready.
    pub fn wait_ready(&self) -> bool {
        let (lock, cvar) = &self.loaded;
        let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
        while !*done {
            done = cvar.wait(done).unwrap_or_else(|e| e.into_inner());
        }
        self.state() == READY
    }
}

/// Start loading the engine from `config`.
pub fn start(config: PathBuf) -> Arc<Shared> {
    let (tx, rx) = channel::<Job>();
    let shared = Arc::new(Shared { jobs: Mutex::new(tx), state: AtomicU8::new(LOADING), loaded: (Mutex::new(false), Condvar::new()) });
    let me = Arc::clone(&shared);
    thread::spawn(move || {
        let started = std::time::Instant::now();
        let loaded = Engine::load(&config, true, None);
        let done = |state: u8| {
            me.state.store(state, Ordering::Release);
            let (lock, cvar) = &me.loaded;
            *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
            cvar.notify_all();
        };
        let engine = match loaded {
            Ok(engine) => Rc::new(engine),
            Err(e) => {
                log(&format!("the engine did not load from {}: {e}", config.display()));
                done(FAILED);
                return;
            }
        };
        log(&format!("engine loaded in {:.1} s", started.elapsed().as_secs_f64()));
        let layout = bopogram::layout::layout("dachen").expect("the Dachen layout");
        let mut server = Server::new(Rc::clone(&engine), layout, None);
        done(READY);
        let mut unsaved = false;
        loop {
            match rx.recv_timeout(SAVE_AFTER) {
                Ok(job) => {
                    unsaved = true;
                    if let Reply::Line(text) = server.handle_line(&job.line) {
                        if let Some(reply) = job.reply {
                            let _ = reply.send(text);
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if std::mem::take(&mut unsaved) {
                        engine.save_user();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        engine.save_user();
    });
    shared
}

static NEXT_SID: AtomicI64 = AtomicI64::new(1);

/// One input context's way to the engine.
pub struct Client {
    shared: Arc<Shared>,
    sid: i64,
    next_id: u64,
}

impl Client {
    pub fn new(shared: Arc<Shared>) -> Client {
        Client { shared, sid: NEXT_SID.fetch_add(1, Ordering::Relaxed), next_id: 1 }
    }

    fn close(&self) {
        if self.shared.state() == READY {
            let line = serde_json::json!({"op": "close", "sid": self.sid}).to_string();
            self.shared.send(Job { line, reply: None });
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

impl bopogram_protocol::Engine for Client {
    fn request(&mut self, op: &Op) -> Result<Response, Failure> {
        match self.shared.state() {
            LOADING => return Err(Failure::Starting),
            FAILED => return Err(Failure::Unreachable),
            _ => {}
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut request: Value = serde_json::from_str(&op.to_json(id)).map_err(|_| Failure::Rejected)?;
        request["sid"] = self.sid.into();
        let (tx, rx) = channel();
        if !self.shared.send(Job { line: request.to_string(), reply: Some(tx) }) {
            return Err(Failure::Unreachable);
        }
        let line = match rx.recv_timeout(TIMEOUT) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => return Err(Failure::TimedOut),
            Err(RecvTimeoutError::Disconnected) => return Err(Failure::Unreachable),
        };
        let value: Value = serde_json::from_str(&line).map_err(|_| Failure::Rejected)?;
        Response::from_value(&value).ok_or(Failure::Rejected)
    }

    /// A new session: nothing of the old one can reach the new one.
    fn abandon(&mut self) {
        self.close();
        self.sid = NEXT_SID.fetch_add(1, Ordering::Relaxed);
    }
}
