//! Named pipe server: one thread per connection, one session id per connection.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::engine::Router;
use crate::log::log;
use crate::win::PipeListener;

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

fn serve(pipe: File, router: &Router) {
    let (sid, responses) = router.register();
    let mut reader = BufReader::new(&pipe);
    while let Ok(Some(line)) = read_request(&mut reader) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut reply = handle(router, sid, &responses, line);
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

/// An error of the host's own: not the engine refusing one request, but
/// the engine (or the request) not getting through at all.
fn error(request: Option<&Map<String, Value>>, message: &str) -> String {
    bopogram_protocol::host_error(request.and_then(|r| r.get("id")), message)
}

/// Forward one request to the engine under this connection's session id
/// and wait for the matching response.
fn handle(router: &Router, sid: u64, responses: &Receiver<String>, line: &str) -> String {
    let Ok(Value::Object(mut request)) = serde_json::from_str::<Value>(line) else {
        return error(None, "request is not a JSON object");
    };
    // `quit` would stop the engine for every connection.
    if request.get("op").and_then(Value::as_str) == Some("quit") {
        return error(Some(&request), "quit is not allowed");
    }
    // Whatever the client put in `sid` is replaced: a connection can only
    // ever reach its own session.
    request.insert("sid".into(), sid.into());
    // Responses that arrived after an earlier timeout are stale.
    while responses.try_recv().is_ok() {}
    let forwarded = Value::Object(request);
    if !router.send(&forwarded.to_string()) {
        return error(forwarded.as_object(), "engine unavailable");
    }
    match responses.recv_timeout(ENGINE_TIMEOUT) {
        Ok(reply) => reply,
        Err(_) => error(forwarded.as_object(), "engine timeout"),
    }
}
