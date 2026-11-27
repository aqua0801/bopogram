//! The models section: which model pack is installed, the charlm tier, and
//! installing a downloaded pack or going back to the previous one.
//!
//! Everything is done by `bopogram.exe pack ...` (beside this program), the
//! same commands the installer and the Linux scripts use; its last line of
//! output is a JSON object saying what happened. Preparing a pack takes some
//! seconds (checking, unpacking, building the dictionary, the self-test), so
//! it runs on a thread; the window is told when it is done.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use serde_json::Value;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Posted to the window when a pack has been prepared (or failed to be).
pub const WM_PACK_DONE: u32 = WM_APP + 1;

/// The tiers in the order the window shows them.
pub const TIERS: [&str; 4] = ["off", "lite", "standard", "advanced"];
pub const TIER_NAMES: [&str; 4] = ["關閉", "輕量", "標準", "進階"];

/// The result of the last preparation, picked up on `WM_PACK_DONE`.
static PREPARED: Mutex<Option<Result<Value, String>>> = Mutex::new(None);

fn bopogram_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.with_file_name("bopogram.exe");
    exe.exists().then_some(exe)
}

/// Run `bopogram.exe pack ARGS --root ROOT`; the JSON it ends with.
pub fn run(root: &Path, args: &[&str]) -> Result<Value, String> {
    let exe = bopogram_exe().ok_or("找不到 bopogram.exe")?;
    let out = Command::new(exe)
        .arg("pack")
        .args(args)
        .arg("--root")
        .arg(root)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("無法執行 bopogram.exe：{e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("");
    let value: Value = serde_json::from_str(last).map_err(|_| {
        let err = String::from_utf8_lossy(&out.stderr);
        format!("bopogram.exe 沒有回報結果：{}", err.lines().last().unwrap_or(""))
    })?;
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(explain(value.get("error").and_then(Value::as_str).unwrap_or("")));
    }
    Ok(value)
}

/// The engine's English error, with a Chinese explanation for the common ones.
fn explain(error: &str) -> String {
    let why = if error.contains("does not match the manifest") {
        "檔案和清單不符：下載可能不完整或被改過，請重新下載。"
    } else if error.contains("needs program version") {
        "這個模型包需要較新版的程式，請先更新程式。"
    } else if error.contains("not a Bopogram pack") || error.contains("not a zip file") || error.contains("no manifest") {
        "這不是 Bopogram 的模型包。"
    } else if error.contains("self-test failed") {
        "模型包的自我檢查沒有通過，沒有更換。"
    } else if error.contains("no previous model pack") {
        "沒有上一版可以回復。"
    } else if error.contains("engine still running") {
        "引擎還在使用舊的檔案：請先按「重新啟動引擎」，再試一次。"
    } else if error.contains("charlm tier pack") {
        "這是 charlm 等級包，請用「加入等級…」。"
    } else if error.contains("is a model pack") {
        "這是模型包，請用「更新模型包…」。"
    } else {
        "沒有變更。"
    };
    format!("{why}\n\n（{error}）")
}

pub struct Status {
    pub current: Option<String>,
    pub previous: Option<String>,
    pub tier: Option<String>,
    pub tiers: Vec<String>,
}

pub fn status(root: &Path) -> Option<Status> {
    let v = run(root, &["status"]).ok()?;
    let text = |name: &str| v.get(name).and_then(Value::as_str).map(str::to_owned);
    Some(Status {
        current: text("current"),
        previous: text("previous"),
        tier: text("tier"),
        tiers: v
            .get("tiers")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_owned)).collect())
            .unwrap_or_default(),
    })
}

/// Prepare `zip` on a thread; `WM_PACK_DONE` is posted to `window` when done.
pub fn start_prepare(window: HWND, root: PathBuf, zip: PathBuf) {
    let target = window.0 as isize;
    std::thread::spawn(move || {
        let result = run(&root, &["prepare", &zip.to_string_lossy()]);
        *PREPARED.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        unsafe {
            let _ = PostMessageW(Some(HWND(target as *mut _)), WM_PACK_DONE, WPARAM(0), LPARAM(0));
        }
    });
}

pub fn take_prepared() -> Option<Result<Value, String>> {
    PREPARED.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Ask for a .zip file.
pub fn choose_zip(owner: HWND, title: &str) -> Option<PathBuf> {
    let filter: Vec<u16> = "模型包 (*.zip)\0*.zip\0\0".encode_utf16().collect();
    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let mut file = vec![0u16; 1024];
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(0);
    Some(PathBuf::from(String::from_utf16_lossy(&file[..len])))
}
