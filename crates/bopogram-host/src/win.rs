//! The Win32 pieces: current user's SID, single-instance mutex, named pipe.

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::FromRawHandle;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, HANDLE,
    HLOCAL,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, OpenProcessToken};

const PIPE_BUFFER_BYTES: u32 = 64 * 1024;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The current user's SID in string form (`S-1-5-21-...`).
pub fn user_sid() -> io::Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let mut len = 0u32;
        // The first call only reports the size needed.
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        // u64 storage keeps the TOKEN_USER header aligned.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        let info = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr() as *mut c_void),
            len,
            &mut len,
        );
        let _ = CloseHandle(token);
        info?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text)?;
        let sid = text.to_string();
        LocalFree(Some(HLOCAL(text.0 as *mut c_void)));
        sid.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

pub use bopogram_protocol::pipe_name;

/// Take the per-user single-instance mutex. `None` means another host
/// already holds it. The handle is kept for the life of the process.
pub fn single_instance(sid: &str) -> io::Result<Option<HANDLE>> {
    let name = wide(&format!(r"Local\bopogram-host-{sid}"));
    unsafe {
        let handle = CreateMutexW(None, false, PCWSTR(name.as_ptr()))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            return Ok(None);
        }
        Ok(Some(handle))
    }
}

/// The name of the event that asks the host of the user `sid` to stop.
/// The install and switch scripts set it (common.ps1, `Stop-Bopogram`).
pub fn stop_event_name(sid: &str) -> String {
    format!(r"Local\bopogram-host-stop-{sid}")
}

/// Create the stop event and wait for it on a thread of its own; `on_stop`
/// runs once it is set. With the default security only this user (and
/// SYSTEM) may set it; sandboxed processes cannot (no write up).
pub fn on_stop_event(sid: &str, on_stop: impl FnOnce() + Send + 'static) -> io::Result<()> {
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
    let name = wide(&stop_event_name(sid));
    let event = unsafe { CreateEventW(None, true, false, PCWSTR(name.as_ptr()))? };
    // A handle is a plain value; the event lives as long as the process.
    let raw = event.0 as usize;
    std::thread::spawn(move || {
        unsafe { WaitForSingleObject(HANDLE(raw as *mut c_void), INFINITE) };
        on_stop();
    });
    Ok(())
}

/// Ask the host of the user `sid` to stop (it saves the personal model
/// first) and wait up to `wait` for it to be gone. False if no host was
/// running; an error if it did not stop in time.
pub fn stop_host(sid: &str, wait: std::time::Duration) -> io::Result<bool> {
    use windows::Win32::System::Threading::{
        OpenEventW, OpenMutexW, SetEvent, EVENT_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE,
    };
    let event_name = wide(&stop_event_name(sid));
    let mutex_name = wide(&format!(r"Local\bopogram-host-{sid}"));
    let running = || unsafe {
        match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(mutex_name.as_ptr())) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            }
            Err(_) => false,
        }
    };
    if !running() {
        return Ok(false);
    }
    unsafe {
        let event = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(event_name.as_ptr()))?;
        let set = SetEvent(event);
        let _ = CloseHandle(event);
        set?;
    }
    let deadline = std::time::Instant::now() + wait;
    while running() {
        if std::time::Instant::now() > deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "the host did not stop in time"));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(true)
}

/// Make this process's standard error (where the engine writes its warnings)
/// go to `file`. A GUI program has none otherwise.
pub fn set_stderr(file: File) {
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE};
    // The handle stays open for the life of the process.
    let handle = file.into_raw_handle();
    unsafe {
        let _ = SetStdHandle(STD_ERROR_HANDLE, HANDLE(handle));
    }
}

/// A named pipe server that only the given user (and SYSTEM) may open,
/// and only from this machine.
pub struct PipeListener {
    name: Vec<u16>,
    security: PSECURITY_DESCRIPTOR,
    next: HANDLE,
}

/// Is the process at the other end of this pipe connection restricted:
/// an app container, or below medium integrity (a sandboxed process)? Such
/// clients may type, but nothing they type is learned or logged. When it
/// cannot be determined, the answer is yes.
pub fn client_restricted(pipe: &File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, TokenIntegrityLevel, TokenIsAppContainer,
        TOKEN_MANDATORY_LABEL,
    };
    use windows::Win32::System::Pipes::GetNamedPipeClientProcessId;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    /// SECURITY_MANDATORY_MEDIUM_RID
    const MEDIUM: u32 = 0x2000;

    unsafe {
        let mut pid = 0u32;
        if GetNamedPipeClientProcessId(HANDLE(pipe.as_raw_handle()), &mut pid).is_err() {
            return true;
        }
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return true;
        };
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token);
        let _ = CloseHandle(process);
        if opened.is_err() {
            return true;
        }
        let mut container = 0u32;
        let mut len = 0u32;
        let is_container = GetTokenInformation(
            token,
            TokenIsAppContainer,
            Some(&mut container as *mut u32 as *mut c_void),
            4,
            &mut len,
        )
        .map_or(true, |_| container != 0);
        let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut len);
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        let low = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr() as *mut c_void),
            len,
            &mut len,
        )
        .map_or(true, |_| {
            let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
            let sid = label.Label.Sid;
            let count = *GetSidSubAuthorityCount(sid);
            count == 0 || *GetSidSubAuthority(sid, count as u32 - 1) < MEDIUM
        });
        let _ = CloseHandle(token);
        is_container || low
    }
}

// The descriptor is immutable after creation and the handle is only used
// from the accepting thread.
unsafe impl Send for PipeListener {}

impl PipeListener {
    pub fn bind(sid: &str) -> io::Result<Self> {
        // Protected DACL: full access for SYSTEM and for this user; read and
        // write for app containers (Store apps, the Start menu search), whose
        // input methods run inside them, both kinds (AC and the "less
        // privileged" S-1-15-2-2). The low integrity label lets them write;
        // the DACL still admits only this user's processes and containers.
        // The server treats such clients as restricted (see server.rs).
        let sddl = wide(&format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;{sid})(A;;GRGW;;;AC)(A;;GRGW;;;S-1-15-2-2)S:(ML;;NW;;;LW)"
        ));
        let mut security = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut security,
                None,
            )?;
        }
        let mut listener = PipeListener {
            name: wide(&pipe_name(sid)),
            security,
            next: HANDLE::default(),
        };
        // Fails if some other process already created a pipe with this name.
        listener.next = listener.create_instance(true)?;
        Ok(listener)
    }

    fn create_instance(&self, first: bool) -> io::Result<HANDLE> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.security.0,
            bInheritHandle: false.into(),
        };
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(self.name.as_ptr()),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER_BYTES,
                PIPE_BUFFER_BYTES,
                0,
                Some(&attributes),
            )
        };
        if handle.is_invalid() {
            return Err(io::Error::last_os_error());
        }
        Ok(handle)
    }

    /// Wait for the next client. A fresh instance is created before the
    /// connected one is handed out, so the pipe name never disappears.
    pub fn accept(&mut self) -> io::Result<File> {
        let handle = self.next;
        unsafe {
            if let Err(err) = ConnectNamedPipe(handle, None) {
                // The client may connect between creation and this call.
                if err.code() != ERROR_PIPE_CONNECTED.to_hresult() {
                    let _ = CloseHandle(handle);
                    self.next = self.create_instance(false)?;
                    return Err(err.into());
                }
            }
            let file = File::from_raw_handle(handle.0 as _);
            self.next = self.create_instance(false)?;
            Ok(file)
        }
    }
}
