//! Settings window of the Bopogram trial input method.
//!
//! A program of its own, opened from the menu of the mode button; nothing
//! of it is loaded into other applications. It edits two local files:
//!
//! - `%LOCALAPPDATA%\Bopogram\style.txt`: casual or formal, read by the
//!   input method whenever an application gets the focus.
//! - the engine's `config.json` (found through `bopogram-host.json` beside
//!   this program): personal learning and the typing log. The engine reads
//!   it at start, so applying a change there restarts the host.
//!
//! No networking, like everything else.
#![windows_subsystem = "windows"]

mod files;

use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, GetSysColorBrush, CLEARTYPE_QUALITY, COLOR_BTNFACE, DEFAULT_CHARSET, HFONT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    FindWindowW, GetDlgItem, GetMessageW, GetWindowLongPtrW, IsDialogMessageW, LoadCursorW,
    MessageBoxW, PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage, CW_USEDEFAULT,
    GWLP_USERDATA, HMENU, IDC_ARROW, MB_ICONWARNING, MB_OK, MSG, SW_SHOW, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_COMMAND, WM_DESTROY, WM_SETFONT, WM_TIMER, WNDCLASSW, WS_CAPTION,
    WS_CHILD, WS_GROUP, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use files::{Files, Settings, DEFAULT_FONT_PX, FONT_SIZES};

const CLASS_NAME: PCWSTR = w!("BopogramSettings");
const TITLE: &str = "Bopogram 注音設定";

// Button styles and messages (winuser.h).
const BS_DEFPUSHBUTTON: u32 = 0x1;
const BS_AUTOCHECKBOX: u32 = 0x3;
const BS_GROUPBOX: u32 = 0x7;
const BS_AUTORADIOBUTTON: u32 = 0x9;
const BM_GETCHECK: u32 = 0xF0;
const BM_SETCHECK: u32 = 0xF1;

// Control identifiers. 1 and 2 are what Enter and Escape send in a dialog.
const ID_OK: i32 = 1;
const ID_CANCEL: i32 = 2;
const ID_APPLY: i32 = 3;
const ID_CASUAL: i32 = 100;
const ID_FORMAL: i32 = 101;
const ID_LEARNING: i32 = 102;
const ID_TYPING_LOG: i32 = 103;
const ID_REVIEW: i32 = 110;
const ID_FOLDER: i32 = 111;
const ID_RESTART: i32 = 112;
const ID_STATUS: i32 = 120;
const ID_TIMING: i32 = 121;
/// One per entry of `FONT_SIZES`.
const ID_FONT: i32 = 130;
const STATUS_TIMER: usize = 1;

/// Client area in pixels at 96 DPI.
const WIDTH: i32 = 460;
const HEIGHT: i32 = 482;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct App {
    files: Files,
    /// What the files held when they were last read or written.
    saved: Settings,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn message(owner: HWND, text: &str) {
    unsafe {
        MessageBoxW(Some(owner), PCWSTR(wide(text).as_ptr()), PCWSTR(wide(TITLE).as_ptr()), MB_OK | MB_ICONWARNING);
    }
}

struct Builder {
    window: HWND,
    font: HFONT,
    dpi: i32,
}

impl Builder {
    fn px(&self, value: i32) -> i32 {
        value * self.dpi / 96
    }

    #[allow(clippy::too_many_arguments)]
    fn control(&self, class: PCWSTR, text: &str, style: u32, id: i32, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            let created = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                PCWSTR(wide(text).as_ptr()),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
                self.px(x),
                self.px(y),
                self.px(w),
                self.px(h),
                Some(self.window),
                Some(HMENU(id as isize as *mut c_void)),
                None,
                None,
            );
            if let Ok(control) = created {
                SendMessageW(control, WM_SETFONT, Some(WPARAM(self.font.0 as usize)), Some(LPARAM(1)));
            }
        }
    }

    fn group(&self, text: &str, y: i32, h: i32) {
        self.control(w!("BUTTON"), text, BS_GROUPBOX, -1, 12, y, WIDTH - 24, h);
    }

    fn label(&self, text: &str, id: i32, x: i32, y: i32, w: i32) {
        self.control(w!("STATIC"), text, 0, id, x, y, w, 20);
    }

    fn button(&self, text: &str, id: i32, x: i32, y: i32, w: i32) {
        let default = if id == ID_OK { BS_DEFPUSHBUTTON } else { 0 };
        self.control(w!("BUTTON"), text, WS_TABSTOP.0 | default, id, x, y, w, 28);
    }

    fn option(&self, text: &str, kind: u32, extra: u32, id: i32, y: i32) {
        self.control(w!("BUTTON"), text, WS_TABSTOP.0 | kind | extra, id, 26, y, WIDTH - 52, 24);
    }
}

fn build(window: HWND, app: &App) {
    let dpi = unsafe { GetDpiForSystem() } as i32;
    let font = unsafe {
        CreateFontW(
            -(15 * dpi / 96),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            Default::default(),
            Default::default(),
            CLEARTYPE_QUALITY,
            0,
            w!("Microsoft JhengHei UI"),
        )
    };
    let b = Builder { window, font, dpi };

    b.group("選字風格", 10, 84);
    b.option("口語：輕聲「ㄉㄜ」一律先選「的」", BS_AUTORADIOBUTTON, WS_GROUP.0, ID_CASUAL, 32);
    b.option("正式：的／地／得由模型判斷", BS_AUTORADIOBUTTON, 0, ID_FORMAL, 58);

    b.group("候選視窗文字大小", 104, 56);
    for (i, name) in ["小", "中", "大", "特大"].iter().enumerate() {
        let style = WS_TABSTOP.0 | BS_AUTORADIOBUTTON | if i == 0 { WS_GROUP.0 } else { 0 };
        b.control(w!("BUTTON"), name, style, ID_FONT + i as i32, 26 + 100 * i as i32, 126, 90, 24);
    }

    b.group("學習與紀錄", 166, 112);
    b.option("個人化學習：記住我常用與改選過的詞", BS_AUTOCHECKBOX, WS_GROUP.0, ID_LEARNING, 188);
    b.option("打字紀錄：記下引擎首選與我的修改，審核後才匯出", BS_AUTOCHECKBOX, 0, ID_TYPING_LOG, 214);
    b.button("審核打字紀錄…", ID_REVIEW, 26, 242, 140);
    b.button("開啟資料夾", ID_FOLDER, 176, 242, 110);

    b.group("引擎", 288, 114);
    let bundle = app.files.config.as_deref().and_then(Path::parent);
    let bundle = bundle.map_or("找不到（bopogram-host.json）".to_owned(), |p| p.display().to_string());
    b.label(&format!("執行包：{bundle}"), -1, 26, 310, WIDTH - 52);
    b.label("", ID_STATUS, 26, 338, 270);
    b.button("重新啟動引擎", ID_RESTART, WIDTH - 26 - 126, 332, 126);
    b.option("計時紀錄（只記耗時，24 小時後自動關閉）", BS_AUTOCHECKBOX, WS_GROUP.0, ID_TIMING, 366);

    b.label("學習與紀錄的變更會在套用時重新啟動引擎。", -1, 14, 412, WIDTH - 28);
    b.button("確定", ID_OK, WIDTH - 12 - 80 * 3 - 16, 442, 80);
    b.button("取消", ID_CANCEL, WIDTH - 12 - 80 * 2 - 8, 442, 80);
    b.button("套用", ID_APPLY, WIDTH - 12 - 80, 442, 80);
}

fn item(window: HWND, id: i32) -> Option<HWND> {
    unsafe { GetDlgItem(Some(window), id).ok() }
}

fn checked(window: HWND, id: i32) -> bool {
    item(window, id)
        .is_some_and(|control| unsafe { SendMessageW(control, BM_GETCHECK, None, None) }.0 == 1)
}

fn set_checked(window: HWND, id: i32, on: bool) {
    if let Some(control) = item(window, id) {
        unsafe { SendMessageW(control, BM_SETCHECK, Some(WPARAM(on as usize)), None) };
    }
}

fn show(window: HWND, settings: &Settings) {
    set_checked(window, ID_CASUAL, !settings.formal);
    set_checked(window, ID_FORMAL, settings.formal);
    set_checked(window, ID_LEARNING, settings.learning);
    set_checked(window, ID_TYPING_LOG, settings.typing_log);
    set_checked(window, ID_TIMING, settings.timing);
    // The offered size nearest to the one set.
    let nearest = FONT_SIZES
        .iter()
        .enumerate()
        .min_by_key(|(_, px)| (**px - settings.font_px).abs())
        .map_or(1, |(i, _)| i);
    for i in 0..FONT_SIZES.len() {
        set_checked(window, ID_FONT + i as i32, i == nearest);
    }
}

fn read(window: HWND) -> Settings {
    Settings {
        formal: checked(window, ID_FORMAL),
        learning: checked(window, ID_LEARNING),
        typing_log: checked(window, ID_TYPING_LOG),
        timing: checked(window, ID_TIMING),
        font_px: (0..FONT_SIZES.len())
            .find(|&i| checked(window, ID_FONT + i as i32))
            .map_or(DEFAULT_FONT_PX, |i| FONT_SIZES[i]),
    }
}

fn show_status(window: HWND, app: &App) {
    let text = if files::host_running(&app.files.host_exe) { "狀態：執行中" } else { "狀態：未執行（打字時自動啟動）" };
    if let Some(control) = item(window, ID_STATUS) {
        unsafe {
            let _ = SetWindowTextW(control, PCWSTR(wide(text).as_ptr()));
        }
    }
}

fn restart_engine(window: HWND, app: &App) {
    files::stop_host(&app.files.host_exe);
    if let Err(err) = Command::new(&app.files.host_exe).creation_flags(CREATE_NO_WINDOW).spawn() {
        message(window, &format!("無法啟動引擎：{err}"));
    }
    show_status(window, app);
}

/// Write what the window shows. False if something could not be saved.
fn apply(window: HWND, app: &mut App) -> bool {
    let wanted = read(window);
    if wanted == app.saved {
        return true;
    }
    let engine_changed =
        wanted.learning != app.saved.learning || wanted.typing_log != app.saved.typing_log;
    if let Err(err) = app.files.save(&wanted, engine_changed) {
        message(window, &format!("設定無法儲存：{err}"));
        return false;
    }
    app.saved = wanted;
    if engine_changed {
        restart_engine(window, app);
    }
    true
}

fn open_review(window: HWND, app: &App) {
    let (Some(python), Some(tool), Some(userdata)) =
        (app.files.python_windowed(), app.files.review_tool.as_ref(), app.files.userdata())
    else {
        message(window, "找不到審核工具（review_log.py）或 Python。");
        return;
    };
    let started =
        Command::new(python).arg(tool).arg(userdata).creation_flags(CREATE_NO_WINDOW).spawn();
    if let Err(err) = started {
        message(window, &format!("無法開啟審核工具：{err}"));
    }
}

fn open_folder(window: HWND, app: &App) {
    match app.files.userdata() {
        Some(folder) if folder.exists() => {
            let _ = Command::new("explorer.exe").arg(folder).spawn();
        }
        _ => message(window, "資料夾還不存在：開始打字後才會建立。"),
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let app = unsafe { (GetWindowLongPtrW(window, GWLP_USERDATA) as *mut App).as_mut() };
    match (msg, app) {
        (WM_COMMAND, Some(app)) => {
            match (wparam.0 & 0xFFFF) as i32 {
                ID_OK => {
                    if apply(window, app) {
                        unsafe {
                            let _ = DestroyWindow(window);
                        }
                    }
                }
                ID_CANCEL => unsafe {
                    let _ = DestroyWindow(window);
                },
                ID_APPLY => {
                    apply(window, app);
                }
                ID_REVIEW => open_review(window, app),
                ID_FOLDER => open_folder(window, app),
                ID_RESTART => restart_engine(window, app),
                _ => {}
            }
            LRESULT(0)
        }
        (WM_TIMER, Some(app)) => {
            show_status(window, app);
            LRESULT(0)
        }
        (WM_DESTROY, _) => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(window, msg, wparam, lparam) },
    }
}

fn main() {
    unsafe {
        // One window is enough: bring the open one to the front.
        if let Ok(open) = FindWindowW(CLASS_NAME, PCWSTR::null()) {
            let _ = SetForegroundWindow(open);
            return;
        }
        let Ok(instance) = GetModuleHandleW(None) else { return };
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            lpszClassName: CLASS_NAME,
            ..WNDCLASSW::default()
        });

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let dpi = GetDpiForSystem() as i32;
        let mut frame = RECT { left: 0, top: 0, right: WIDTH * dpi / 96, bottom: HEIGHT * dpi / 96 };
        let _ = AdjustWindowRectEx(&mut frame, style, false, WINDOW_EX_STYLE(0));
        let Ok(window) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            PCWSTR(wide(TITLE).as_ptr()),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            frame.right - frame.left,
            frame.bottom - frame.top,
            None,
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };

        let files = Files::locate();
        let saved = files.load();
        let app = Box::into_raw(Box::new(App { files, saved }));
        build(window, &*app);
        show(window, &(*app).saved);
        show_status(window, &*app);
        SetWindowLongPtrW(window, GWLP_USERDATA, app as isize);
        SetTimer(Some(window), STATUS_TIMER, 2000, None);
        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetForegroundWindow(window);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // Tab, Enter and Escape as in a dialog box.
            if !IsDialogMessageW(window, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        drop(Box::from_raw(app));
    }
}
