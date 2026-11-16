//! The files behind the settings window, and the host process.

use std::path::{Path, PathBuf};

use serde_json::Value;
use windows::core::PWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE,
};

const CASUAL: &str = "casual";
const FORMAL: &str = "formal";
/// Where the engine keeps these files when the config does not say.
const DEFAULT_USER_MODEL: &str = "userdata/user.json";
const DEFAULT_TYPING_LOG: &str = "userdata/typing_log.jsonl";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Settings {
    pub formal: bool,
    /// The engine keeps a personal model (`user_model` in config.json).
    pub learning: bool,
    /// The engine records committed compositions (`typing_log`).
    pub typing_log: bool,
}

pub struct Files {
    pub host_exe: PathBuf,
    /// The Python that runs the engine.
    pub python: Option<PathBuf>,
    /// The engine's config.json.
    pub config: Option<PathBuf>,
    pub review_tool: Option<PathBuf>,
    /// The style chosen by the user, shared with the input method.
    pub style: Option<PathBuf>,
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

impl Files {
    /// Everything is found from `bopogram-host.json` beside this program.
    pub fn locate() -> Self {
        let folder = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        let host = read_json(&folder.join("bopogram-host.json")).unwrap_or(Value::Null);
        let path = |name: &str| host.get(name).and_then(Value::as_str).map(PathBuf::from);
        let python = path("python");
        // <prototype>\.venv\Scripts\python.exe -> <prototype>\tools\review_log.py
        let review_tool = path("review_tool")
            .or_else(|| Some(python.as_ref()?.ancestors().nth(3)?.join("tools").join("review_log.py")))
            .filter(|tool| tool.exists());
        Files {
            host_exe: folder.join("bopogram-host.exe"),
            python,
            config: path("config"),
            review_tool,
            style: std::env::var_os("LOCALAPPDATA")
                .map(|local| PathBuf::from(local).join("Bopogram").join("style.txt")),
        }
    }

    /// `pythonw.exe` beside the engine's Python if there is one (no console window).
    pub fn python_windowed(&self) -> Option<PathBuf> {
        let python = self.python.as_ref()?;
        let windowed = python.with_file_name("pythonw.exe");
        Some(if windowed.exists() { windowed } else { python.clone() })
    }

    pub fn userdata(&self) -> Option<PathBuf> {
        Some(self.config.as_ref()?.parent()?.join("userdata"))
    }

    pub fn load(&self) -> Settings {
        let config = self.config.as_deref().and_then(read_json).unwrap_or(Value::Null);
        let set = |name: &str| config.get(name).is_some_and(|value| !value.is_null());
        let style = self
            .style
            .as_deref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| text.trim().to_owned())
            .filter(|style| style == CASUAL || style == FORMAL)
            .or_else(|| config.get("style")?.as_str().map(str::to_owned));
        Settings {
            formal: style.as_deref() == Some(FORMAL),
            learning: set("user_model"),
            typing_log: set("typing_log"),
        }
    }

    /// Write the settings. The engine's config is only touched when `engine` is true.
    pub fn save(&self, settings: &Settings, engine: bool) -> Result<(), String> {
        let style = self.style.as_deref().ok_or("找不到 %LOCALAPPDATA%")?;
        if let Some(folder) = style.parent() {
            std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        }
        std::fs::write(style, if settings.formal { FORMAL } else { CASUAL })
            .map_err(|e| format!("{}: {e}", style.display()))?;
        if !engine {
            return Ok(());
        }

        let path = self.config.as_deref().ok_or("找不到引擎的 config.json")?;
        let mut config = read_json(path).ok_or_else(|| format!("{} 無法讀取", path.display()))?;
        let object = config.as_object_mut().ok_or("config.json 格式不符")?;
        // Keep the path the config already names; only switch it on or off.
        let mut switch = |name: &str, on: bool, default: &str| {
            let present = object.get(name).is_some_and(|value| !value.is_null());
            if on && !present {
                object.insert(name.to_owned(), Value::from(default));
            } else if !on && present {
                object.insert(name.to_owned(), Value::Null);
            }
        };
        switch("user_model", settings.learning, DEFAULT_USER_MODEL);
        switch("typing_log", settings.typing_log, DEFAULT_TYPING_LOG);
        let text = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
        // Written beside the file and renamed, so a failure cannot leave half a config.
        let temporary = path.with_extension("json.new");
        std::fs::write(&temporary, text).map_err(|e| format!("{}: {e}", temporary.display()))?;
        std::fs::rename(&temporary, path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Process ids of the running copies of `exe`.
fn processes(exe: &Path) -> Vec<u32> {
    let name = exe.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase());
    let mut found = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return found;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..PROCESSENTRY32W::default()
        };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
            let file = String::from_utf16_lossy(&entry.szExeFile[..len]).to_ascii_lowercase();
            if Some(&file) == name.as_ref() && image_is(entry.th32ProcessID, exe) {
                found.push(entry.th32ProcessID);
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    found
}

fn image_is(pid: u32, exe: &Path) -> bool {
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let named =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        named.is_ok()
            && String::from_utf16_lossy(&buf[..len as usize])
                .eq_ignore_ascii_case(&exe.to_string_lossy())
    }
}

pub fn host_running(host_exe: &Path) -> bool {
    !processes(host_exe).is_empty()
}

/// End our own host (and with it the engine). Waits briefly for it to go,
/// so that a new one can take over the pipe.
pub fn stop_host(host_exe: &Path) {
    for pid in processes(host_exe) {
        unsafe {
            if let Ok(process) = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) {
                let _ = TerminateProcess(process, 0);
                WaitForSingleObject(process, 3000);
                let _ = CloseHandle(process);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(folder: &Path) -> Files {
        Files {
            host_exe: folder.join("bopogram-host.exe"),
            python: None,
            config: Some(folder.join("config.json")),
            review_tool: None,
            style: Some(folder.join("local").join("style.txt")),
        }
    }

    #[test]
    fn load_and_save() {
        let folder = std::env::temp_dir().join(format!("bopogram-settings-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("config.json"),
            r#"{"dict": "dict.tsv", "beam": 16, "style": "formal",
                "user_model": "userdata/mine.json", "typing_log": null}"#,
        )
        .unwrap();
        let files = files(&folder);
        // No style file yet: the config's default counts.
        let loaded = files.load();
        assert_eq!(loaded, Settings { formal: true, learning: true, typing_log: false });

        // Style alone does not touch the config.
        let before = std::fs::read_to_string(folder.join("config.json")).unwrap();
        files.save(&Settings { formal: false, ..loaded.clone() }, false).unwrap();
        assert_eq!(std::fs::read_to_string(folder.join("config.json")).unwrap(), before);
        assert!(!files.load().formal);

        let wanted = Settings { formal: false, learning: false, typing_log: true };
        files.save(&wanted, true).unwrap();
        assert_eq!(files.load(), wanted);
        let config = read_json(&folder.join("config.json")).unwrap();
        assert!(config["user_model"].is_null());
        assert_eq!(config["typing_log"], DEFAULT_TYPING_LOG);
        assert_eq!(config["beam"], 16, "other keys are kept");

        // Switching back on keeps a path the config still names, else the default.
        files.save(&Settings { learning: true, ..wanted }, true).unwrap();
        assert_eq!(read_json(&folder.join("config.json")).unwrap()["user_model"], DEFAULT_USER_MODEL);
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
