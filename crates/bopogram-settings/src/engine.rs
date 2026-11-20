//! The engine, reached through the host's pipe like the input method does:
//! the dictionary window reads and changes the user's dictionary this way,
//! because only the engine writes those files.
//!
//! Requests block until the host answers. The host answers within its own
//! engine timeout, so the window freezes for at most that long.

use std::ffi::c_void;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use bopogram_protocol::{pipe_name, HOST_ERROR, STARTING};
use serde_json::Value;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Do not start the host more often than this.
const LAUNCH_INTERVAL: Duration = Duration::from_secs(10);
/// Win32 error: every instance of the pipe is busy.
const ERROR_PIPE_BUSY: i32 = 231;

pub enum Reply {
    /// The engine did it; the whole response.
    Done(Value),
    /// The engine (or the host) said no: the `reason` and the `error` text.
    Refused(String, String),
    /// The engine is loading, or the host was just started: ask again shortly.
    Starting,
    /// Nobody to talk to.
    Unreachable(String),
}

pub struct Engine {
    host_exe: PathBuf,
    pipe: Option<File>,
    next_id: u64,
    launched: Option<Instant>,
}

impl Engine {
    pub fn new(host_exe: PathBuf) -> Self {
        Engine { host_exe, pipe: None, next_id: 1, launched: None }
    }

    /// Send `{"op": ..., ...}` and wait for the answer.
    pub fn request(&mut self, mut request: Value) -> Reply {
        request["id"] = self.next_id.into();
        self.next_id += 1;
        let pipe = match self.connect() {
            Ok(pipe) => pipe,
            Err(reply) => return reply,
        };
        let line = request.to_string() + "\n";
        let mut answer = String::new();
        let mut writer = pipe;
        let exchanged =
            writer.write_all(line.as_bytes()).and_then(|()| BufReader::new(pipe).read_line(&mut answer));
        if !matches!(exchanged, Ok(n) if n > 0) {
            self.pipe = None;
            return Reply::Unreachable("連線中斷".to_owned());
        }
        let Ok(value) = serde_json::from_str::<Value>(&answer) else {
            return Reply::Unreachable("回應無法解讀".to_owned());
        };
        let text = |name: &str| value.get(name).and_then(Value::as_str).unwrap_or("").to_owned();
        if value.get(STARTING).is_some() {
            return Reply::Starting;
        }
        if value.get(HOST_ERROR).is_some() {
            self.pipe = None;
            return Reply::Unreachable(text("error"));
        }
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            return Reply::Refused(text("reason"), text("error"));
        }
        Reply::Done(value)
    }

    /// The open pipe, opening it (and starting the host) if need be.
    fn connect(&mut self) -> Result<&File, Reply> {
        if self.pipe.is_none() {
            let Some(sid) = user_sid() else {
                return Err(Reply::Unreachable("讀不到使用者身分".to_owned()));
            };
            match std::fs::OpenOptions::new().read(true).write(true).open(pipe_name(&sid)) {
                Ok(pipe) => self.pipe = Some(pipe),
                Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) => return Err(Reply::Starting),
                Err(_) => {
                    // No host: start one, and wait for it.
                    if self.launched.is_none_or(|at| at.elapsed() > LAUNCH_INTERVAL) {
                        self.launched = Some(Instant::now());
                        if let Err(err) = Command::new(&self.host_exe).creation_flags(CREATE_NO_WINDOW).spawn() {
                            return Err(Reply::Unreachable(format!("無法啟動引擎：{err}")));
                        }
                    }
                    return Err(Reply::Starting);
                }
            }
        }
        self.pipe.as_ref().ok_or(Reply::Starting)
    }
}

/// The current user's SID, as the host names its pipe after it.
fn user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        let read = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as *mut c_void), len, &mut len);
        let _ = CloseHandle(token);
        read.ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text).ok()?;
        let sid = text.to_string().ok();
        LocalFree(Some(HLOCAL(text.0 as *mut c_void)));
        sid
    }
}
