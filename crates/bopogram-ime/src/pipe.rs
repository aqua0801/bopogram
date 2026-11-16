//! Named pipe client for the engine host.
//!
//! This runs on the application's UI thread, inside the key handler, so
//! every wait has a deadline. If the host cannot be reached in time the
//! request fails and the key is left to the application.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_IO_PENDING, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE,
    HANDLE, HLOCAL, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    GetTokenInformation, TokenElevationType, TokenElevationTypeFull, TokenIsAppContainer,
    TokenUser, TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING,
    SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows::Win32::System::Pipes::{GetNamedPipeServerProcessId, WaitNamedPipeW};
use windows::Win32::System::Threading::{
    CreateEventW, CreateProcessW, GetCurrentProcess, OpenProcess, OpenProcessToken,
    QueryFullProcessImageNameW, WaitForSingleObject, CREATE_NO_WINDOW, DETACHED_PROCESS,
    PROCESS_INFORMATION, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
};
use windows::Win32::System::IO::{CancelIo, GetOverlappedResult, OVERLAPPED};

use crate::protocol::{Engine, Op, Response};

/// Longest wait for one request, from the handoff notes.
const REQUEST_TIMEOUT: Duration = Duration::from_millis(200);
/// Wait for a free pipe instance when all are momentarily busy.
const BUSY_WAIT_MS: u32 = 50;
/// Do not start the host more often than this.
const LAUNCH_INTERVAL: Duration = Duration::from_secs(5);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// May this process start our helper programs (the host, the settings
/// window)? See `Token::may_launch_host`.
pub fn may_launch() -> bool {
    Token::current().is_some_and(|token| token.may_launch_host())
}

/// Start a program without waiting for it and without handing it any of
/// the application's handles.
pub fn spawn(exe: &Path) {
    let application = wide(&exe.to_string_lossy());
    let mut command = wide(&format!("\"{}\"", exe.to_string_lossy()));
    let directory = exe.parent().map(|dir| wide(&dir.to_string_lossy()));
    let startup =
        STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..STARTUPINFOW::default() };
    let mut process = PROCESS_INFORMATION::default();
    unsafe {
        let started = CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NO_WINDOW | DETACHED_PROCESS,
            None,
            directory.as_ref().map_or(PCWSTR::null(), |dir| PCWSTR(dir.as_ptr())),
            &startup,
            &mut process,
        );
        if started.is_ok() {
            let _ = CloseHandle(process.hProcess);
            let _ = CloseHandle(process.hThread);
        }
    }
}

struct Token(HANDLE);

impl Token {
    fn current() -> Option<Self> {
        let mut handle = HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle).ok()? };
        Some(Token(handle))
    }

    fn user_sid(&self) -> Option<String> {
        unsafe {
            let mut len = 0u32;
            let _ = GetTokenInformation(self.0, TokenUser, None, 0, &mut len);
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            GetTokenInformation(self.0, TokenUser, Some(buf.as_mut_ptr() as *mut c_void), len, &mut len)
                .ok()?;
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut text = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut text).ok()?;
            let sid = text.to_string().ok();
            LocalFree(Some(HLOCAL(text.0 as *mut c_void)));
            sid
        }
    }

    /// A 32-bit piece of token information; `None` if it cannot be read.
    fn dword(&self, class: TOKEN_INFORMATION_CLASS) -> Option<u32> {
        let mut value = 0u32;
        let mut len = 0u32;
        unsafe {
            GetTokenInformation(
                self.0,
                class,
                Some(&mut value as *mut _ as *mut c_void),
                std::mem::size_of::<u32>() as u32,
                &mut len,
            )
            .ok()?;
        }
        Some(value)
    }

    /// May this process start the host? The host must run as the user's
    /// ordinary desktop process, so two kinds of process must not start it:
    ///
    /// - An app container (Store apps, the Start menu): the host would be
    ///   locked inside the container, one copy per app, unreachable by
    ///   everything else.
    /// - A process elevated through UAC while the user's other programs are
    ///   not: they could not open the elevated host's pipe. Where UAC does
    ///   not split the token (everything runs with the same rights), there
    ///   is no such gap and starting the host is fine.
    ///
    /// When in doubt the answer is no.
    fn may_launch_host(&self) -> bool {
        let app_container = self.dword(TokenIsAppContainer);
        let elevation = self.dword(TokenElevationType);
        app_container == Some(0)
            && elevation.is_some_and(|kind| kind != TokenElevationTypeFull.0 as u32)
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Full path of the executable behind a process id.
fn process_image(pid: u32) -> Option<PathBuf> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let found =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        found.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

struct Connection {
    pipe: HANDLE,
    event: HANDLE,
    /// Bytes received after the last complete line.
    buffered: Vec<u8>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.pipe);
            let _ = CloseHandle(self.event);
        }
    }
}

impl Connection {
    fn open(name: &[u16], host_exe: Option<&Path>) -> Option<Self> {
        let open = || unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                // The server may learn who we are but may not act as us.
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            )
        };
        let pipe = match open() {
            Ok(pipe) => pipe,
            Err(err) if err.code() == ERROR_PIPE_BUSY.to_hresult() => {
                if !unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), BUSY_WAIT_MS) }.as_bool() {
                    return None;
                }
                open().ok()?
            }
            Err(_) => return None,
        };
        let event = match unsafe { CreateEventW(None, true, false, PCWSTR::null()) } {
            Ok(event) => event,
            Err(_) => {
                unsafe {
                    let _ = CloseHandle(pipe);
                }
                return None;
            }
        };
        let connection = Connection { pipe, event, buffered: Vec::new() };
        // Only talk to our own host: the pipe name alone proves nothing.
        if let Some(expected) = host_exe {
            let mut pid = 0u32;
            unsafe { GetNamedPipeServerProcessId(pipe, &mut pid).ok()? };
            if !same_path(&process_image(pid)?, expected) {
                return None;
            }
        }
        Some(connection)
    }

    /// Finish an overlapped operation, giving up at the deadline.
    fn complete(
        &self,
        started: windows::core::Result<()>,
        overlapped: &OVERLAPPED,
        deadline: Instant,
    ) -> Option<usize> {
        let mut transferred = 0u32;
        unsafe {
            if let Err(err) = started {
                if err.code() != ERROR_IO_PENDING.to_hresult() {
                    return None;
                }
                let left = deadline.saturating_duration_since(Instant::now()).as_millis() as u32;
                if WaitForSingleObject(self.event, left) != WAIT_OBJECT_0 {
                    // The buffer must not be touched by the kernel after we return.
                    let _ = CancelIo(self.pipe);
                    let _ = GetOverlappedResult(self.pipe, overlapped, &mut transferred, true);
                    return None;
                }
            }
            GetOverlappedResult(self.pipe, overlapped, &mut transferred, false).ok()?;
        }
        Some(transferred as usize)
    }

    fn write(&self, mut data: &[u8], deadline: Instant) -> Option<()> {
        while !data.is_empty() {
            let overlapped = OVERLAPPED { hEvent: self.event, ..OVERLAPPED::default() };
            let started = unsafe {
                WriteFile(self.pipe, Some(data), None, Some(&overlapped as *const _ as *mut _))
            };
            let written = self.complete(started, &overlapped, deadline)?;
            data = &data[written..];
        }
        Some(())
    }

    fn read_line(&mut self, deadline: Instant) -> Option<Vec<u8>> {
        loop {
            if let Some(end) = self.buffered.iter().position(|&b| b == b'\n') {
                let mut line: Vec<u8> = self.buffered.drain(..=end).collect();
                line.pop();
                return Some(line);
            }
            let mut chunk = [0u8; 4096];
            let overlapped = OVERLAPPED { hEvent: self.event, ..OVERLAPPED::default() };
            let started = unsafe {
                ReadFile(self.pipe, Some(&mut chunk), None, Some(&overlapped as *const _ as *mut _))
            };
            let read = self.complete(started, &overlapped, deadline)?;
            if read == 0 {
                return None;
            }
            self.buffered.extend_from_slice(&chunk[..read]);
        }
    }

    fn round_trip(&mut self, line: &str, id: u64, deadline: Instant) -> Option<Response> {
        self.write(format!("{line}\n").as_bytes(), deadline)?;
        loop {
            let reply = self.read_line(deadline)?;
            let value: Value = serde_json::from_slice(&reply).ok()?;
            // Anything else is the late answer to a request we gave up on.
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return Response::from_value(&value);
            }
        }
    }
}

pub struct PipeEngine {
    pipe_name: Vec<u16>,
    /// The host executable: started when the pipe is absent, and the only
    /// program we accept as the pipe's server. `None` skips both.
    host_exe: Option<PathBuf>,
    /// False in processes that must not start the host (see Token::may_launch_host).
    may_launch: bool,
    connection: Option<Connection>,
    next_id: u64,
    last_launch: Option<Instant>,
}

impl PipeEngine {
    /// `None` for accounts that never run a host (SYSTEM and the service
    /// accounts, which is also what the sign-in screen runs as).
    pub fn new(host_exe: Option<PathBuf>) -> Option<Self> {
        let token = Token::current()?;
        let sid = token.user_sid()?;
        if matches!(sid.as_str(), "S-1-5-18" | "S-1-5-19" | "S-1-5-20") {
            return None;
        }
        Some(PipeEngine {
            pipe_name: wide(&format!(r"\\.\pipe\bopogram-{sid}")),
            host_exe,
            may_launch: token.may_launch_host(),
            connection: None,
            next_id: 1,
            last_launch: None,
        })
    }

    /// Connect, or start the host if nobody is listening. Never waits for
    /// the host to come up; call this early (when the input method is
    /// activated) so the host is ready by the first key.
    pub fn connect(&mut self) -> bool {
        if self.connection.is_none() {
            self.connection = Connection::open(&self.pipe_name, self.host_exe.as_deref());
            if self.connection.is_none() {
                self.launch_host();
            }
        }
        self.connection.is_some()
    }

    fn launch_host(&mut self) {
        let Some(exe) = self.host_exe.as_deref().filter(|_| self.may_launch) else {
            return;
        };
        if self.last_launch.is_some_and(|at| at.elapsed() < LAUNCH_INTERVAL) {
            return;
        }
        self.last_launch = Some(Instant::now());
        spawn(exe);
    }
}

impl Engine for PipeEngine {
    fn request(&mut self, op: &Op) -> Option<Response> {
        if !self.connect() {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let response = self.connection.as_mut()?.round_trip(&op.to_json(id), id, deadline);
        if response.is_none() {
            // The connection may be out of step with the host; a new one
            // gets a new, empty session.
            self.connection = None;
        }
        response
    }
}
