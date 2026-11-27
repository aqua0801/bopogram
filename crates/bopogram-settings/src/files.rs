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

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub formal: bool,
    /// The engine keeps a personal model (`user_model` in config.json).
    pub learning: bool,
    /// The engine records committed compositions (`typing_log`).
    pub typing_log: bool,
    /// The input method logs how long each key takes (never what was typed).
    pub timing: bool,
    /// Candidate window text size, in pixels at 96 DPI.
    pub font_px: i32,
    /// One Chinese/English mode for all applications, instead of one each.
    pub shared_mode: bool,
    /// Candidates per page (5 to 10).
    pub page_size: usize,
    /// Pick candidates with A S D F ... instead of 1 2 3 ...
    pub home_row: bool,
    /// New applications start in Chinese mode.
    pub start_chinese: bool,
    /// Shift on its own switches Chinese/English.
    pub shift_toggles: bool,
    /// Opens the phrase menu; `None` turns the menu off.
    pub phrase_key: Option<Hotkey>,
    /// While composing: add the composition to the dictionary.
    pub dict_add_key: Option<Hotkey>,
    /// In the candidate window: remove the highlighted candidate.
    pub dict_remove_key: Option<Hotkey>,
}

/// A key combination, as the input method reads it from `keys.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub vk: u32,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Alt+`.
pub const DEFAULT_PHRASE_KEY: Hotkey = Hotkey { vk: 0xC0, ctrl: false, alt: true, shift: false };
/// Ctrl+= and Ctrl+-.
pub const DEFAULT_DICT_ADD_KEY: Hotkey = Hotkey { vk: 0xBB, ctrl: true, alt: false, shift: false };
pub const DEFAULT_DICT_REMOVE_KEY: Hotkey = Hotkey { vk: 0xBD, ctrl: true, alt: false, shift: false };

impl Hotkey {
    fn parse(value: &Value) -> Option<Hotkey> {
        let vk = u32::try_from(value.get("vk")?.as_u64()?).ok()?;
        let flag = |name: &str| value.get(name).and_then(Value::as_bool).unwrap_or(false);
        let key = Hotkey { vk, ctrl: flag("ctrl"), alt: flag("alt"), shift: flag("shift") };
        ((1..0xFF).contains(&vk) && (key.ctrl || key.alt)).then_some(key)
    }

    fn to_json(self) -> Value {
        serde_json::json!({ "vk": self.vk, "ctrl": self.ctrl, "alt": self.alt, "shift": self.shift })
    }
}

/// One entry of the phrase menu (`phrases.json`). Lines end with `\n`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Phrase {
    pub name: String,
    pub text: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            formal: false,
            learning: false,
            typing_log: false,
            timing: false,
            font_px: DEFAULT_FONT_PX,
            shared_mode: false,
            page_size: DEFAULT_PAGE_SIZE,
            home_row: false,
            start_chinese: true,
            shift_toggles: true,
            phrase_key: Some(DEFAULT_PHRASE_KEY),
            dict_add_key: Some(DEFAULT_DICT_ADD_KEY),
            dict_remove_key: Some(DEFAULT_DICT_REMOVE_KEY),
        }
    }
}

/// Candidates per page offered, and the one used when nothing is set.
pub const PAGE_SIZES: [usize; 4] = [5, 7, 9, 10];
pub const DEFAULT_PAGE_SIZE: usize = 9;

/// The text sizes offered, and the one used when nothing is set.
pub const FONT_SIZES: [i32; 4] = [16, 18, 22, 26];
pub const DEFAULT_FONT_PX: i32 = 18;

/// How long the timing log stays on once turned on.
const TIMING_HOURS: u64 = 24;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[derive(Clone)]
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

impl Files {
    /// Holds the time (Unix seconds) until which the input method logs timings.
    fn timing_switch(&self) -> Option<PathBuf> {
        Some(self.style.as_ref()?.with_file_name("timing-until.txt"))
    }

    /// The candidate window's text size, read by the input method.
    fn font_file(&self) -> Option<PathBuf> {
        Some(self.style.as_ref()?.with_file_name("candidate-font.txt"))
    }

    /// `global` when the mode is shared, read by the input method.
    fn mode_scope_file(&self) -> Option<PathBuf> {
        Some(self.style.as_ref()?.with_file_name("mode-scope.txt"))
    }

    /// Selection keys, page size, start mode and Shift, read by the input method.
    fn keys_file(&self) -> Option<PathBuf> {
        Some(self.style.as_ref()?.with_file_name("keys.json"))
    }

    /// The phrase menu, read by the input method each time it opens.
    fn phrases_file(&self) -> Option<PathBuf> {
        Some(self.style.as_ref()?.with_file_name("phrases.json"))
    }

    pub fn load_phrases(&self) -> Vec<Phrase> {
        let Some(value) = self.phrases_file().as_deref().and_then(read_json) else {
            return Vec::new();
        };
        let field = |item: &Value, name: &str| {
            item.get(name).and_then(Value::as_str).unwrap_or("").replace("\r\n", "\n")
        };
        value
            .get("phrases")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|item| Phrase { name: field(item, "name"), text: field(item, "text") })
                    .filter(|phrase| !phrase.text.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Write the phrase menu. Phrases without text are dropped.
    pub fn save_phrases(&self, phrases: &[Phrase]) -> Result<(), String> {
        let path = self.phrases_file().ok_or("找不到 %LOCALAPPDATA%")?;
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        }
        let list: Vec<Value> = phrases
            .iter()
            .filter(|phrase| !phrase.text.is_empty())
            .map(|phrase| serde_json::json!({ "name": phrase.name, "text": phrase.text }))
            .collect();
        let text = serde_json::to_string_pretty(&serde_json::json!({ "phrases": list }))
            .map_err(|e| e.to_string())?;
        // Written beside the file and renamed, so the menu never reads half a file.
        let temporary = path.with_extension("json.new");
        std::fs::write(&temporary, text).map_err(|e| format!("{}: {e}", temporary.display()))?;
        std::fs::rename(&temporary, &path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The data folder when the models were installed from a model pack: the
    /// engine's config is then `<root>\models\engine.json` and the user's data
    /// is in `<root>\userdata` (`bopogram::pack`).
    pub fn pack_root(&self) -> Option<PathBuf> {
        let config = self.config.as_deref()?;
        let models = config.parent()?;
        let is_pack = config.file_name()? == "engine.json" && models.file_name()? == "models";
        is_pack.then(|| models.parent().map(Path::to_path_buf)).flatten()
    }

    /// Where the user's files go, relative to the engine's config.
    fn user_default(&self, file: &str) -> String {
        let name = file.trim_start_matches("userdata/");
        if self.pack_root().is_some() { format!("../userdata/{name}") } else { format!("userdata/{name}") }
    }

    /// The engine's personal model, whether or not learning is on now.
    pub fn user_model(&self) -> Option<PathBuf> {
        let config = self.config.as_deref()?;
        let named = read_json(config)
            .and_then(|c| c.get("user_model")?.as_str().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(self.user_default(DEFAULT_USER_MODEL)));
        Some(if named.is_absolute() { named } else { config.parent()?.join(named) })
    }

    /// Delete the personal model (the host must be stopped first, or it
    /// writes the model back). True if there was one.
    pub fn clear_user_model(&self) -> Result<bool, String> {
        let model = self.user_model().ok_or("找不到引擎的 config.json")?;
        let mut found = false;
        // The engine writes a .tmp beside it first, then renames it.
        for path in [model.clone(), model.with_extension("tmp")] {
            if path.exists() {
                std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                found = true;
            }
        }
        Ok(found)
    }

    /// Until when the timing log is on, if it is.
    pub fn timing_until(&self) -> Option<u64> {
        let text = std::fs::read_to_string(self.timing_switch()?).ok()?;
        text.trim().parse::<u64>().ok().filter(|&until| until > now_secs())
    }
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
        let path = |name: &str| {
            host.get(name).and_then(Value::as_str).map(|v| PathBuf::from(bopogram_protocol::expand_env(v)))
        };
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
        if let Some(root) = self.pack_root() {
            return Some(root.join("userdata"));
        }
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
            timing: self.timing_until().is_some(),
            font_px: self
                .font_file()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .and_then(|text| text.trim().parse().ok())
                .filter(|px| (12..=40).contains(px))
                .unwrap_or(DEFAULT_FONT_PX),
            shared_mode: self
                .mode_scope_file()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .is_some_and(|text| text.trim() == "global"),
            ..self.load_keys()
        }
    }

    /// The fields kept in `keys.json`; the rest are defaults.
    fn load_keys(&self) -> Settings {
        let mut settings = Settings::default();
        let Some(keys) = self.keys_file().as_deref().and_then(read_json) else {
            return settings;
        };
        if let Some(size) = keys.get("page_size").and_then(Value::as_u64) {
            if (5..=10).contains(&size) {
                settings.page_size = size as usize;
            }
        }
        settings.home_row = keys.get("select_keys").and_then(Value::as_str) == Some("home_row");
        settings.start_chinese = keys.get("start_mode").and_then(Value::as_str) != Some("english");
        settings.shift_toggles = keys.get("shift_toggle").and_then(Value::as_bool) != Some(false);
        // Null turns the key off; something unreadable keeps the default.
        let hotkey = |name: &str, default: Hotkey| match keys.get(name) {
            Some(Value::Null) => None,
            Some(key) => Hotkey::parse(key).or(Some(default)),
            None => Some(default),
        };
        settings.phrase_key = hotkey("phrase_key", DEFAULT_PHRASE_KEY);
        settings.dict_add_key = hotkey("dict_add_key", DEFAULT_DICT_ADD_KEY);
        settings.dict_remove_key = hotkey("dict_remove_key", DEFAULT_DICT_REMOVE_KEY);
        settings
    }

    /// Write the settings. The engine's config is only touched when `engine` is true.
    pub fn save(&self, settings: &Settings, engine: bool) -> Result<(), String> {
        let style = self.style.as_deref().ok_or("找不到 %LOCALAPPDATA%")?;
        if let Some(folder) = style.parent() {
            std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        }
        std::fs::write(style, if settings.formal { FORMAL } else { CASUAL })
            .map_err(|e| format!("{}: {e}", style.display()))?;
        if let Some(scope) = self.mode_scope_file() {
            std::fs::write(&scope, if settings.shared_mode { "global" } else { "application" })
                .map_err(|e| format!("{}: {e}", scope.display()))?;
        }
        if let Some(keys) = self.keys_file() {
            let text = serde_json::json!({
                "page_size": settings.page_size,
                "select_keys": if settings.home_row { "home_row" } else { "digits" },
                "start_mode": if settings.start_chinese { "chinese" } else { "english" },
                "shift_toggle": settings.shift_toggles,
                "phrase_key": settings.phrase_key.map_or(Value::Null, Hotkey::to_json),
                "dict_add_key": settings.dict_add_key.map_or(Value::Null, Hotkey::to_json),
                "dict_remove_key": settings.dict_remove_key.map_or(Value::Null, Hotkey::to_json),
            });
            std::fs::write(&keys, text.to_string()).map_err(|e| format!("{}: {e}", keys.display()))?;
        }
        if let Some(font) = self.font_file() {
            std::fs::write(&font, settings.font_px.to_string())
                .map_err(|e| format!("{}: {e}", font.display()))?;
        }
        if let Some(switch) = self.timing_switch() {
            let on = self.timing_until().is_some();
            if settings.timing && !on {
                let until = now_secs() + TIMING_HOURS * 3600;
                std::fs::write(&switch, until.to_string())
                    .map_err(|e| format!("{}: {e}", switch.display()))?;
            } else if !settings.timing && switch.exists() {
                std::fs::remove_file(&switch).map_err(|e| format!("{}: {e}", switch.display()))?;
            }
        }
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
        switch("user_model", settings.learning, &self.user_default(DEFAULT_USER_MODEL));
        switch("typing_log", settings.typing_log, &self.user_default(DEFAULT_TYPING_LOG));
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
    fn model_pack_layout() {
        let root = std::env::temp_dir().join(format!("bopogram-settings-pack-{}", std::process::id()));
        std::fs::create_dir_all(root.join("models")).unwrap();
        std::fs::write(root.join("models").join("engine.json"), r#"{"user_model": null, "typing_log": null}"#).unwrap();
        let bundle = files(&root);
        let files = Files { config: Some(root.join("models").join("engine.json")), ..bundle.clone() };
        assert_eq!(files.pack_root().as_deref(), Some(root.as_path()));
        assert_eq!(files.userdata(), Some(root.join("userdata")));
        // Learning switched on: the personal model goes into the data folder's userdata.
        files.save(&Settings { learning: true, ..Settings::default() }, true).unwrap();
        let config = read_json(&root.join("models").join("engine.json")).unwrap();
        assert_eq!(config["user_model"], "../userdata/user.json");
        assert_eq!(files.user_model(), Some(root.join("models").join("../userdata/user.json")));
        // A runtime bundle is not a pack.
        assert_eq!(bundle.pack_root(), None);
        std::fs::remove_dir_all(&root).unwrap();
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
        assert_eq!(
            loaded,
            Settings {
                formal: true,
                learning: true,
                typing_log: false,
                ..Settings::default()
            }
        );

        // Style alone does not touch the config.
        let before = std::fs::read_to_string(folder.join("config.json")).unwrap();
        files.save(&Settings { formal: false, ..loaded.clone() }, false).unwrap();
        assert_eq!(std::fs::read_to_string(folder.join("config.json")).unwrap(), before);
        assert!(!files.load().formal);

        let wanted = Settings {
            formal: false,
            learning: false,
            typing_log: true,
            timing: true,
            font_px: 22,
            shared_mode: true,
            page_size: 7,
            home_row: true,
            start_chinese: false,
            shift_toggles: false,
            phrase_key: Some(Hotkey { vk: 0x51, ctrl: true, alt: false, shift: true }),
            dict_add_key: Some(Hotkey { vk: 0x41, ctrl: true, alt: true, shift: false }),
            dict_remove_key: None,
        };
        files.save(&wanted, true).unwrap();
        assert_eq!(files.load(), wanted);
        files.save(&Settings { phrase_key: None, ..wanted.clone() }, false).unwrap();
        assert_eq!(files.load().phrase_key, None, "the phrase menu turned off");
        let until = files.timing_until().unwrap();
        assert!(until > now_secs() + 23 * 3600 && until <= now_secs() + 24 * 3600);
        files.save(&Settings { timing: false, ..wanted.clone() }, false).unwrap();
        assert!(!files.load().timing);
        files.save(&wanted, false).unwrap();
        let config = read_json(&folder.join("config.json")).unwrap();
        assert!(config["user_model"].is_null());
        assert_eq!(config["typing_log"], DEFAULT_TYPING_LOG);
        assert_eq!(config["beam"], 16, "other keys are kept");

        // Switching back on keeps a path the config still names, else the default.
        files.save(&Settings { learning: true, ..wanted }, true).unwrap();
        assert_eq!(read_json(&folder.join("config.json")).unwrap()["user_model"], DEFAULT_USER_MODEL);

        // Clearing the personal model removes it and its temporary file.
        let model = files.user_model().unwrap();
        assert_eq!(model, folder.join(DEFAULT_USER_MODEL));
        assert!(!files.clear_user_model().unwrap(), "nothing yet");
        std::fs::create_dir_all(model.parent().unwrap()).unwrap();
        std::fs::write(&model, "{}").unwrap();
        std::fs::write(model.with_extension("tmp"), "{}").unwrap();
        assert!(files.clear_user_model().unwrap());
        assert!(!model.exists() && !model.with_extension("tmp").exists());

        // Phrases: line breaks kept, empty ones dropped.
        assert!(files.load_phrases().is_empty());
        let phrases = [
            Phrase { name: "簽名".into(), text: "祝好\n小明".into() },
            Phrase { name: "空的".into(), text: String::new() },
            Phrase { name: String::new(), text: "地址".into() },
        ];
        files.save_phrases(&phrases).unwrap();
        assert_eq!(files.load_phrases(), [phrases[0].clone(), phrases[2].clone()]);
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
