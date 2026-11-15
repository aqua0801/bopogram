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

pub fn pipe_name(sid: &str) -> String {
    format!(r"\\.\pipe\bopogram-{sid}")
}

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

/// A named pipe server that only the given user (and SYSTEM) may open,
/// and only from this machine.
pub struct PipeListener {
    name: Vec<u16>,
    security: PSECURITY_DESCRIPTOR,
    next: HANDLE,
}

// The descriptor is immutable after creation and the handle is only used
// from the accepting thread.
unsafe impl Send for PipeListener {}

impl PipeListener {
    pub fn bind(sid: &str) -> io::Result<Self> {
        // Protected DACL: full access for SYSTEM and for this user, nobody else.
        let sddl = wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})"));
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
