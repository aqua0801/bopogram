//! Named pipe server: one thread per connection, one session id per connection.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bopogram_protocol::{host_error, host_starting};
use serde_json::{Map, Value};

use crate::engine::Router;
use crate::log::log;
use crate::win::{self, PipeListener};

/// Longest request line accepted from a client.
const MAX_LINE_BYTES: usize = 64 * 1024;
/// How long a connection waits for the engine to answer one request.
const ENGINE_TIMEOUT: Duration = Duration::from_secs(5);

pub fn run(mut listener: PipeListener, router: Arc<Router>) -> ! {
    let mut failures = 0;
    loop {
        match listener.accept() {
            Ok(pipe) => {
                failures = 0;
                let router = Arc::clone(&router);
                thread::spawn(move || serve(pipe, &router));
            }
            Err(err) => {
                log!("pipe accept failed: {err}");
                failures += 1;
                if failures >= 5 {
                    log!("pipe keeps failing; host is stopping");
                    std::process::exit(1);
                }
            }
        }
    }
}

/// One client connection.
struct Connection<'a> {
    router: &'a Router,
    sid: u64,
    responses: Receiver<String>,
    /// An app container or a sandboxed process: learning stays off.
    restricted: bool,
    /// The engine session has been told that learning is off.
    learning_off_sent: bool,
}

fn serve(pipe: File, router: &Router) {
    let (sid, responses) = router.register();
    let restricted = win::client_restricted(&pipe);
    if restricted {
        log!("restricted client (app container or low integrity): learning off");
    }
    let mut connection =
        Connection { router, sid, responses, restricted, learning_off_sent: false };
    let mut reader = BufReader::new(&pipe);
    while let Ok(Some(line)) = read_request(&mut reader) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut reply = connection.handle(line);
        reply.push('\n');
        if (&pipe).write_all(reply.as_bytes()).is_err() {
            break;
        }
    }
    router.unregister(sid);
}

/// Read one request line. `Ok(None)` when the client is gone. An overlong
/// line is an error, which ends the connection.
fn read_request(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = reader
        .take(MAX_LINE_BYTES as u64 + 1)
        .read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') && n > MAX_LINE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "request too long"));
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Operations that change the user's dictionary: refused to restricted
/// clients, like learning (crates/docs/userdict-handoff.md).
const DICT_CHANGES: &[&str] = &["dict_add", "dict_remove", "dict_exclude", "dict_restore", "dict_reload"];

/// The host refusing one request, as the engine would: the session is fine.
fn refused(id: Option<&Value>) -> String {
    let mut reply = serde_json::json!({
        "ok": false,
        "error": "this program may not change the dictionary",
        "reason": "restricted",
    });
    if let Some(id) = id {
        reply["id"] = id.clone();
    }
    reply.to_string()
}

/// An error of the host's own: not the engine refusing one request, but
/// the engine (or the request) not getting through at all.
fn error(request: Option<&Map<String, Value>>, message: &str) -> String {
    host_error(request.and_then(|r| r.get("id")), message)
}

impl Connection<'_> {
    /// Forward one request to the engine under this connection's session id
    /// and wait for the matching response.
    fn handle(&mut self, line: &str) -> String {
        let Ok(Value::Object(mut request)) = serde_json::from_str::<Value>(line) else {
            return error(None, "request is not a JSON object");
        };
        // `quit` would stop the engine for every connection.
        if request.get("op").and_then(Value::as_str) == Some("quit") {
            return error(Some(&request), "quit is not allowed");
        }
        if self.restricted {
            // Nor may it change the user's dictionary (reading it is fine).
            let op = request.get("op").and_then(Value::as_str).unwrap_or("");
            if DICT_CHANGES.contains(&op) {
                return refused(request.get("id"));
            }
            // Whatever the client asks, nothing it types is learned.
            if request.get("op").and_then(Value::as_str) == Some("learning") {
                request.insert("enabled".into(), false.into());
            }
            if !self.learning_off_sent {
                let off = serde_json::json!({"op": "learning", "enabled": false, "id": "host"});
                if self.forward(off.as_object().cloned().unwrap_or_default()).is_none() {
                    return host_starting(request.get("id"));
                }
                self.learning_off_sent = true;
            }
        }
        let id = request.get("id").cloned();
        self.forward(request).unwrap_or_else(|| host_starting(id.as_ref()))
    }

    /// Send a request and wait for the response with the same `id`. `None`
    /// if the engine is not running (loading or restarting).
    fn forward(&self, mut request: Map<String, Value>) -> Option<String> {
        // Whatever the client put in `sid` is replaced: a connection can
        // only ever reach its own session.
        request.insert("sid".into(), self.sid.into());
        // Responses that arrived after an earlier timeout are stale.
        while self.responses.try_recv().is_ok() {}
        let id = request.get("id").cloned();
        let forwarded = Value::Object(request);
        if !self.router.send(&forwarded.to_string()) {
            return None;
        }
        let deadline = Instant::now() + ENGINE_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.responses.recv_timeout(left) {
                Ok(reply) => {
                    let answered = serde_json::from_str::<Value>(&reply)
                        .ok()
                        .and_then(|v| v.get("id").cloned());
                    if answered == id {
                        return Some(reply);
                    }
                }
                Err(_) => return Some(error(forwarded.as_object(), "engine timeout")),
            }
        }
    }
}
