//! Settings window of the Bopogram input method.
//!
//! A program of its own, opened from the menu of the mode button; nothing
//! of it is loaded into other applications. It edits two local files:
//!
//! - `%LOCALAPPDATA%\Bopogram\*`: the input method's own settings (style,
//!   keys, candidate text size, Chinese/English mode), read by the input
//!   method in every application within a few seconds.
//! - the engine's `config.json` (found through `bopogram-host.json` beside
//!   this program): personal learning and the typing log. The engine reads
//!   it at start, so applying a change there restarts the host.
//!
//! No networking, like everything else.
#![windows_subsystem = "windows"]

mod dictionary;
mod engine;
mod files;
mod models;
mod phrases;

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
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, TOOLTIPS_CLASSW, TTF_IDISHWND,
    TTF_SUBCLASS, TTM_ADDTOOLW, TTM_SETMAXTIPWIDTH, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    FindWindowW, GetAncestor, GetDlgItem, GetMessageW, GetWindowLongPtrW, IsDialogMessageW, LoadCursorW, GA_ROOT,
    MessageBoxW, PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage, CW_USEDEFAULT,
    GWLP_USERDATA, HMENU, IDC_ARROW, IDYES, MB_DEFBUTTON2, MB_ICONINFORMATION, MB_ICONWARNING,
    MB_OK, MB_YESNO, MSG, SW_SHOW, WINDOW_EX_STYLE, GetSystemMetrics, LoadImageW, ICON_BIG,
    ICON_SMALL, IMAGE_ICON, LR_LOADFROMFILE, SM_CXICON, SM_CXSMICON, WM_SETICON,
    WINDOW_STYLE, WM_COMMAND, WM_DESTROY, WM_SETFONT, WM_TIMER, WNDCLASSW, WS_CAPTION,
    WS_CHILD, WS_GROUP, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use files::{Files, Hotkey, Settings, DEFAULT_FONT_PX, DEFAULT_PAGE_SIZE, FONT_SIZES, PAGE_SIZES};

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
const ID_SHARED_MODE: i32 = 140;
const ID_START_CHINESE: i32 = 141;
const ID_START_ENGLISH: i32 = 142;
const ID_SHIFT: i32 = 143;
const ID_DIGIT_KEYS: i32 = 150;
const ID_HOME_ROW: i32 = 151;
/// One per entry of `PAGE_SIZES`.
const ID_PAGE: i32 = 160;
const ID_CLEAR: i32 = 170;
const ID_PHRASE_KEY: i32 = 180;
const ID_PHRASES: i32 = 181;
const ID_DICT_ADD_KEY: i32 = 182;
const ID_DICT_REMOVE_KEY: i32 = 183;
const ID_DICTIONARY: i32 = 184;
const ID_MODEL_INFO: i32 = 190;
/// One per entry of `models::TIERS`.
const ID_TIER: i32 = 191;
const ID_PACK_UPDATE: i32 = 196;
const ID_TIER_ADD: i32 = 197;
const ID_ROLLBACK: i32 = 198;
const STATUS_TIMER: usize = 1;

// The hot key control (commctrl.h): a field that shows the key combination pressed in it.
const HOTKEY_CLASS: PCWSTR = w!("msctls_hotkey32");
const HKM_SETHOTKEY: u32 = 0x401;
const HKM_GETHOTKEY: u32 = 0x402;
const HKM_SETRULES: u32 = 0x403;
const HOTKEYF_SHIFT: u32 = 0x1;
const HOTKEYF_CONTROL: u32 = 0x2;
const HOTKEYF_ALT: u32 = 0x4;
const HKCOMB_NONE: usize = 0x1;
const HKCOMB_S: usize = 0x2;
const WS_BORDER: u32 = 0x0080_0000;

/// Client area in pixels at 96 DPI: two columns.
const WIDTH: i32 = 780;
const HEIGHT: i32 = 640;
/// Where the OK, Cancel and Apply buttons sit.
const BUTTONS_Y: i32 = 600;
const COLUMN: i32 = 372;
const LEFT: i32 = 12;
const RIGHT: i32 = LEFT + COLUMN + 12;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct App {
    files: Files,
    /// What the files held when they were last read or written.
    saved: Settings,
    /// The charlm tier in use (installed from a model pack only).
    tier: Option<String>,
    /// A model pack is being prepared.
    busy: bool,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn message(owner: HWND, text: &str) {
    unsafe {
        MessageBoxW(Some(owner), PCWSTR(wide(text).as_ptr()), PCWSTR(wide(TITLE).as_ptr()), MB_OK | MB_ICONWARNING);
    }
}

fn notice(owner: HWND, text: &str) {
    unsafe {
        MessageBoxW(Some(owner), PCWSTR(wide(text).as_ptr()), PCWSTR(wide(TITLE).as_ptr()), MB_OK | MB_ICONINFORMATION);
    }
}

/// A question answered Yes or No; No is the default.
fn ask(owner: HWND, text: &str) -> bool {
    unsafe {
        MessageBoxW(
            Some(owner),
            PCWSTR(wide(text).as_ptr()),
            PCWSTR(wide(TITLE).as_ptr()),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        ) == IDYES
    }
}

struct Builder {
    window: HWND,
    font: HFONT,
    dpi: i32,
    /// Shows the longer explanation of a control while the mouse rests on it.
    tooltip: Option<HWND>,
}

impl Builder {
    fn px(&self, value: i32) -> i32 {
        value * self.dpi / 96
    }

    #[allow(clippy::too_many_arguments)]
    fn control(&self, class: PCWSTR, text: &str, style: u32, id: i32, x: i32, y: i32, w: i32, h: i32) -> Option<HWND> {
        self.control_ex(0, class, text, style, id, x, y, w, h)
    }

    #[allow(clippy::too_many_arguments)]
    fn control_ex(
        &self,
        ex: u32,
        class: PCWSTR,
        text: &str,
        style: u32,
        id: i32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> Option<HWND> {
        unsafe {
            let created = CreateWindowExW(
                WINDOW_EX_STYLE(ex),
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
            let control = created.ok()?;
            SendMessageW(control, WM_SETFONT, Some(WPARAM(self.font.0 as usize)), Some(LPARAM(1)));
            Some(control)
        }
    }

    /// The explanation shown when the mouse rests on control `id`.
    fn tip(&self, id: i32, text: &str) {
        let (Some(tooltip), Some(control)) = (self.tooltip, item(self.window, id)) else {
            return;
        };
        let mut text = wide(text);
        let info = TTTOOLINFOW {
            // Without the last field: the size every version of the
            // common controls accepts.
            cbSize: (std::mem::size_of::<TTTOOLINFOW>() - std::mem::size_of::<*mut c_void>()) as u32,
            uFlags: TTF_IDISHWND | TTF_SUBCLASS,
            hwnd: self.window,
            uId: control.0 as usize,
            lpszText: windows::core::PWSTR(text.as_mut_ptr()),
            ..Default::default()
        };
        unsafe { SendMessageW(tooltip, TTM_ADDTOOLW, None, Some(LPARAM(&info as *const _ as isize))) };
    }

    /// A frame around a column's section; `x` is `LEFT` or `RIGHT`.
    fn group(&self, text: &str, x: i32, y: i32, h: i32) {
        self.control(w!("BUTTON"), text, BS_GROUPBOX, -1, x, y, COLUMN, h);
    }

    fn label(&self, text: &str, id: i32, x: i32, y: i32, w: i32) {
        self.control(w!("STATIC"), text, 0, id, x, y, w, 20);
    }

    fn button(&self, text: &str, id: i32, x: i32, y: i32, w: i32) {
        let default = if id == ID_OK { BS_DEFPUSHBUTTON } else { 0 };
        self.control(w!("BUTTON"), text, WS_TABSTOP.0 | default, id, x, y, w, 28);
    }

    /// A check box or radio button across the section at `x`.
    fn option(&self, text: &str, kind: u32, extra: u32, id: i32, x: i32, y: i32) {
        self.control(w!("BUTTON"), text, WS_TABSTOP.0 | kind | extra, id, x + 14, y, COLUMN - 28, 24);
    }

    /// Radio buttons side by side; the first starts a group.
    fn radios(&self, names: &[&str], first_id: i32, x: i32, y: i32, width: i32) {
        for (i, name) in names.iter().enumerate() {
            let style = WS_TABSTOP.0 | BS_AUTORADIOBUTTON | if i == 0 { WS_GROUP.0 } else { 0 };
            self.control(w!("BUTTON"), name, style, first_id + i as i32, x + width * i as i32, y, width - 6, 24);
        }
    }
}

/// The text of every window here.
fn ui_font(dpi: i32) -> HFONT {
    unsafe {
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
    }
}

fn build(window: HWND, app: &App) {
    let dpi = unsafe { GetDpiForSystem() } as i32;
    let font = ui_font(dpi);
    let tooltip = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            TOOLTIPS_CLASSW,
            PCWSTR::null(),
            WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(window),
            None,
            None,
            None,
        )
        .ok()
    };
    if let Some(tooltip) = tooltip {
        // Wrap long explanations.
        unsafe { SendMessageW(tooltip, TTM_SETMAXTIPWIDTH, None, Some(LPARAM((360 * dpi / 96) as isize))) };
    }
    let b = Builder { window, font, dpi, tooltip };
    let (l, r) = (LEFT, RIGHT);

    // Left column: choosing characters.
    b.group("選字風格", l, 10, 58);
    b.radios(&["口語", "正式"], ID_CASUAL, l + 14, 32, 110);
    b.tip(ID_CASUAL, "輕聲「ㄉㄜ」一律先選「的」。");
    b.tip(ID_FORMAL, "「的／地／得」由模型依上下文判斷。");

    b.group("候選視窗", l, 78, 118);
    b.label("選字鍵", -1, l + 14, 104, 84);
    b.radios(&["1 2 3 …", "A S D F …"], ID_DIGIT_KEYS, l + 100, 100, 110);
    b.tip(ID_HOME_ROW, "候選視窗開著時用 A S D F … 選字，數字鍵照常是注音。");
    b.label("每頁候選數", -1, l + 14, 134, 84);
    let pages: Vec<String> = PAGE_SIZES.iter().map(usize::to_string).collect();
    let pages: Vec<&str> = pages.iter().map(String::as_str).collect();
    b.radios(&pages, ID_PAGE, l + 100, 130, 56);
    b.label("文字大小", -1, l + 14, 164, 84);
    b.radios(&["小", "中", "大", "特大"], ID_FONT, l + 100, 160, 56);

    b.group("學習與紀錄", l, 206, 128);
    b.option("個人化學習", BS_AUTOCHECKBOX, WS_GROUP.0, ID_LEARNING, l, 228);
    b.tip(ID_LEARNING, "記住常用與改選過的詞，只存在本機。變更時會重新啟動引擎。");
    b.option("打字紀錄", BS_AUTOCHECKBOX, 0, ID_TYPING_LOG, l, 254);
    b.tip(ID_TYPING_LOG, "記下首選與你的修改，用來改進選字；只存在本機，審核後才匯出。變更時會重新啟動引擎。");
    b.button("審核紀錄…", ID_REVIEW, l + 14, 288, 110);
    b.button("開啟資料夾", ID_FOLDER, l + 132, 288, 110);
    b.button("清除學習…", ID_CLEAR, l + 250, 288, 108);
    b.tip(ID_CLEAR, "刪除個人化學習資料（無法復原）。打字紀錄與詞庫不受影響。");

    // Right column: modes and the engine.
    b.group("中／英模式", r, 10, 118);
    b.option("所有程式共用", BS_AUTOCHECKBOX, WS_GROUP.0, ID_SHARED_MODE, r, 32);
    b.tip(ID_SHARED_MODE, "所有程式共用同一個中／英模式；新開的程式沿用目前的模式。");
    b.label("新開的程式", -1, r + 14, 66, 84);
    b.radios(&["中文", "英文"], ID_START_CHINESE, r + 100, 62, 80);
    b.option("單按 Shift 切換中／英", BS_AUTOCHECKBOX, WS_GROUP.0, ID_SHIFT, r, 92);
    b.tip(ID_SHIFT, "中文模式下 Shift+空白鍵另外切換全形／半形。");

    // A key field. A key without Ctrl or Alt would be taken from typing:
    // `added` is put with it.
    let hotkey = |id: i32, x: i32, y: i32, added: u32| {
        if let Some(key) = b.control(HOTKEY_CLASS, "", WS_TABSTOP.0 | WS_BORDER, id, x, y, 160, 26) {
            unsafe {
                SendMessageW(key, HKM_SETRULES, Some(WPARAM(HKCOMB_NONE | HKCOMB_S)), Some(LPARAM(added as isize)))
            };
        }
    };

    b.group("詞庫", l, 344, 136);
    b.label("加入詞庫", -1, l + 14, 370, 84);
    hotkey(ID_DICT_ADD_KEY, l + 100, 366, HOTKEYF_CONTROL);
    b.tip(ID_DICT_ADD_KEY, "組字時按下，把整個組字區加入詞庫，讀音取自你打的注音。按 Backspace 清空則停用。");
    b.label("刪除候選", -1, l + 14, 404, 84);
    hotkey(ID_DICT_REMOVE_KEY, l + 100, 400, HOTKEYF_CONTROL);
    b.tip(
        ID_DICT_REMOVE_KEY,
        "候選視窗開著時按兩次，刪除選中的候選：自訂詞直接刪除，字典的詞改為排除。按 Backspace 清空則停用。",
    );
    b.button("管理詞庫…", ID_DICTIONARY, l + 14, 438, 126);
    b.tip(ID_DICTIONARY, "查看自訂詞與排除的詞；加入新詞、排除不想要的詞，或取消排除。");

    b.group("快速輸入", r, 138, 100);
    b.label("片語選單", -1, r + 14, 164, 84);
    hotkey(ID_PHRASE_KEY, r + 100, 160, HOTKEYF_ALT);
    b.tip(ID_PHRASE_KEY, "按下想用的組合鍵（需含 Ctrl 或 Alt）；按 Backspace 清空則停用片語選單。");
    b.button("編輯片語…", ID_PHRASES, r + 14, 198, 126);
    b.tip(ID_PHRASES, "在任何程式按上面的組合鍵叫出片語選單，選了就插入文字（不經過剪貼簿）。");

    b.group("引擎", r, 248, 156);
    let bundle = app.files.config.as_deref().and_then(Path::parent);
    let bundle = bundle.map_or("找不到（bopogram-host.json）".to_owned(), |p| p.display().to_string());
    b.label(&format!("執行包：{bundle}"), -1, r + 14, 270, COLUMN - 28);
    b.label("", ID_STATUS, r + 14, 296, COLUMN - 28);
    b.option("計時紀錄", BS_AUTOCHECKBOX, WS_GROUP.0, ID_TIMING, r, 322);
    b.tip(ID_TIMING, "只記每個按鍵的耗時，不記內容；24 小時後自動關閉。");
    b.button("重新啟動引擎", ID_RESTART, r + 14, 360, 126);

    b.group("模型", r, 414, 176);
    b.label("", ID_MODEL_INFO, r + 14, 436, COLUMN - 28);
    b.label("charlm 等級", -1, r + 14, 468, 84);
    b.radios(&models::TIER_NAMES, ID_TIER, r + 100, 464, 64);
    b.tip(ID_TIER, "選字時的字級語言模型。輕量最快；進階較準但較慢、較耗記憶體。按「套用」後重新啟動引擎。");
    b.button("更新模型包…", ID_PACK_UPDATE, r + 14, 504, 110);
    b.tip(ID_PACK_UPDATE, "選擇下載好的模型包（.zip）：檢查、建立字典、自我檢查都通過才更換，舊版保留。");
    b.button("加入等級…", ID_TIER_ADD, r + 132, 504, 110);
    b.tip(ID_TIER_ADD, "加入輕量或進階級的 charlm 等級包（.zip）。");
    b.button("回到上一版", ID_ROLLBACK, r + 250, 504, 108);
    b.label("模型包需自行下載，輸入法不會連線。", -1, r + 14, 546, COLUMN - 28);

    b.button("確定", ID_OK, WIDTH - 12 - 80 * 3 - 16, BUTTONS_Y, 80);
    b.button("取消", ID_CANCEL, WIDTH - 12 - 80 * 2 - 8, BUTTONS_Y, 80);
    b.button("套用", ID_APPLY, WIDTH - 12 - 80, BUTTONS_Y, 80);
}

/// The input method's icon (`bopogram.ico` beside this program) on the
/// window's title bar and taskbar button.
pub(crate) fn set_icon(window: HWND) {
    let Some(path) = std::env::current_exe().ok().map(|exe| exe.with_file_name("bopogram.ico")) else {
        return;
    };
    let path = wide(&path.to_string_lossy());
    for (kind, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
        unsafe {
            let size = GetSystemMetrics(metric);
            let loaded = LoadImageW(None, PCWSTR(path.as_ptr()), IMAGE_ICON, size, size, LR_LOADFROMFILE);
            if let Ok(icon) = loaded {
                SendMessageW(window, WM_SETICON, Some(WPARAM(kind as usize)), Some(LPARAM(icon.0 as isize)));
            }
        }
    }
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

/// A key into hot key field `id` (nothing: that key is off).
fn show_hotkey(window: HWND, id: i32, key: Option<Hotkey>) {
    let value = key.map_or(0, |key| {
        let mut modifiers = 0;
        if key.shift {
            modifiers |= HOTKEYF_SHIFT;
        }
        if key.ctrl {
            modifiers |= HOTKEYF_CONTROL;
        }
        if key.alt {
            modifiers |= HOTKEYF_ALT;
        }
        key.vk | modifiers << 8
    });
    if let Some(control) = item(window, id) {
        unsafe { SendMessageW(control, HKM_SETHOTKEY, Some(WPARAM(value as usize)), None) };
    }
}

fn read_hotkey(window: HWND, id: i32) -> Option<Hotkey> {
    let control = item(window, id)?;
    let value = unsafe { SendMessageW(control, HKM_GETHOTKEY, None, None) }.0 as u32;
    let (vk, modifiers) = (value & 0xFF, (value >> 8) & 0xFF);
    let key = Hotkey {
        vk,
        ctrl: modifiers & HOTKEYF_CONTROL != 0,
        alt: modifiers & HOTKEYF_ALT != 0,
        shift: modifiers & HOTKEYF_SHIFT != 0,
    };
    (vk != 0 && (key.ctrl || key.alt)).then_some(key)
}

fn show(window: HWND, settings: &Settings) {
    show_hotkey(window, ID_PHRASE_KEY, settings.phrase_key);
    show_hotkey(window, ID_DICT_ADD_KEY, settings.dict_add_key);
    show_hotkey(window, ID_DICT_REMOVE_KEY, settings.dict_remove_key);
    set_checked(window, ID_CASUAL, !settings.formal);
    set_checked(window, ID_FORMAL, settings.formal);
    set_checked(window, ID_LEARNING, settings.learning);
    set_checked(window, ID_TYPING_LOG, settings.typing_log);
    set_checked(window, ID_TIMING, settings.timing);
    set_checked(window, ID_SHARED_MODE, settings.shared_mode);
    set_checked(window, ID_START_CHINESE, settings.start_chinese);
    set_checked(window, ID_START_ENGLISH, !settings.start_chinese);
    set_checked(window, ID_SHIFT, settings.shift_toggles);
    set_checked(window, ID_DIGIT_KEYS, !settings.home_row);
    set_checked(window, ID_HOME_ROW, settings.home_row);
    for (i, size) in PAGE_SIZES.iter().enumerate() {
        set_checked(window, ID_PAGE + i as i32, *size == settings.page_size);
    }
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
        shared_mode: checked(window, ID_SHARED_MODE),
        font_px: (0..FONT_SIZES.len())
            .find(|&i| checked(window, ID_FONT + i as i32))
            .map_or(DEFAULT_FONT_PX, |i| FONT_SIZES[i]),
        page_size: (0..PAGE_SIZES.len())
            .find(|&i| checked(window, ID_PAGE + i as i32))
            .map_or(DEFAULT_PAGE_SIZE, |i| PAGE_SIZES[i]),
        home_row: checked(window, ID_HOME_ROW),
        start_chinese: !checked(window, ID_START_ENGLISH),
        shift_toggles: checked(window, ID_SHIFT),
        phrase_key: read_hotkey(window, ID_PHRASE_KEY),
        dict_add_key: read_hotkey(window, ID_DICT_ADD_KEY),
        dict_remove_key: read_hotkey(window, ID_DICT_REMOVE_KEY),
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

/// The tier chosen in the window, if it differs from the one in use: set it
/// and restart the engine. False if it could not be set.
fn apply_tier(window: HWND, app: &mut App) -> bool {
    let Some(root) = app.files.pack_root() else { return true };
    let chosen = (0..models::TIERS.len()).find(|&i| checked(window, ID_TIER + i as i32)).map(|i| models::TIERS[i]);
    let Some(chosen) = chosen.filter(|c| app.tier.as_deref() != Some(*c)) else { return true };
    match models::run(&root, &["tier", chosen]) {
        Ok(_) => {
            app.tier = Some(chosen.to_owned());
            restart_engine(window, app);
            true
        }
        Err(err) => {
            message(window, &format!("無法切換 charlm 等級：{err}"));
            show_models(window, app);
            false
        }
    }
}

fn enable(window: HWND, id: i32, on: bool) {
    if let Some(control) = item(window, id) {
        unsafe {
            let _ = EnableWindow(control, on);
        }
    }
}

fn set_text(window: HWND, id: i32, text: &str) {
    if let Some(control) = item(window, id) {
        unsafe {
            let _ = SetWindowTextW(control, PCWSTR(wide(text).as_ptr()));
        }
    }
}

/// The installed model pack, the tier, and which buttons can be used.
fn show_models(window: HWND, app: &mut App) {
    let root = app.files.pack_root();
    let status = root.as_deref().and_then(models::status);
    let info = match (&root, &status) {
        (None, _) => "使用執行包（開發用），不支援模型包。".to_owned(),
        (Some(_), None) => "讀不到模型狀態（bopogram.exe）。".to_owned(),
        (Some(_), Some(s)) if app.busy => format!("正在準備模型包…（目前 {}）", s.current.as_deref().unwrap_or("無")),
        (Some(_), Some(s)) => format!(
            "模型版本：{}{}",
            s.current.as_deref().unwrap_or("未安裝"),
            s.previous.as_deref().map_or(String::new(), |p| format!("（上一版 {p}）"))
        ),
    };
    set_text(window, ID_MODEL_INFO, &info);
    app.tier = status.as_ref().and_then(|s| s.tier.clone());
    let usable = root.is_some() && status.is_some() && !app.busy;
    for (i, tier) in models::TIERS.iter().enumerate() {
        let have = status.as_ref().is_some_and(|s| s.tiers.iter().any(|t| t == tier));
        enable(window, ID_TIER + i as i32, usable && have);
        set_checked(window, ID_TIER + i as i32, app.tier.as_deref() == Some(*tier));
    }
    enable(window, ID_PACK_UPDATE, usable);
    enable(window, ID_TIER_ADD, usable && status.as_ref().is_some_and(|s| s.current.is_some()));
    enable(window, ID_ROLLBACK, usable && status.as_ref().is_some_and(|s| s.previous.is_some()));
}

fn update_pack(window: HWND, app: &mut App) {
    let Some(root) = app.files.pack_root() else { return };
    let Some(zip) = models::choose_zip(window, "選擇模型包") else { return };
    app.busy = true;
    show_models(window, app);
    models::start_prepare(window, root, zip);
}

/// The pack is ready in models-new: put it in place and restart the engine.
fn pack_prepared(window: HWND, app: &mut App) {
    app.busy = false;
    let Some(result) = models::take_prepared() else { return };
    let Some(root) = app.files.pack_root() else { return };
    let done = result.and_then(|prepared| models::run(&root, &["switch"]).map(|_| prepared));
    match done {
        Ok(prepared) => {
            restart_engine(window, app);
            let version = prepared.get("version").and_then(serde_json::Value::as_str).unwrap_or("?");
            let fell_back = prepared.get("tier_fell_back").and_then(serde_json::Value::as_bool) == Some(true);
            let extra = if fell_back { "\n\n新的模型包沒有你選的 charlm 等級，已改用標準級。" } else { "" };
            notice(window, &format!("已更新為模型版本 {version}，引擎已重新啟動。{extra}"));
        }
        Err(err) => message(window, &format!("模型包沒有更換：{err}")),
    }
    show_models(window, app);
}

fn add_tier(window: HWND, app: &mut App) {
    let Some(root) = app.files.pack_root() else { return };
    let Some(zip) = models::choose_zip(window, "選擇 charlm 等級包") else { return };
    match models::run(&root, &["add-tier", &zip.to_string_lossy()]) {
        Ok(_) => notice(window, "已加入。在「charlm 等級」選擇它並按「套用」即可使用。"),
        Err(err) => message(window, &format!("沒有加入：{err}")),
    }
    show_models(window, app);
}

fn rollback(window: HWND, app: &mut App) {
    let Some(root) = app.files.pack_root() else { return };
    if !ask(window, "要回到上一版的模型嗎？\n\n目前的模型會保留為「上一版」，之後可以再切回來。個人資料不受影響。") {
        return;
    }
    match models::run(&root, &["rollback"]) {
        Ok(_) => {
            restart_engine(window, app);
            notice(window, "已回到上一版的模型，引擎已重新啟動。");
        }
        Err(err) => message(window, &format!("沒有變更：{err}")),
    }
    show_models(window, app);
}

/// Forget everything learned: stop the engine (or it would write the model
/// back), delete the model, start the engine again.
fn clear_personal(window: HWND, app: &App) {
    let question = "要清除個人化學習資料嗎？\n\n引擎記住的常用詞與改選紀錄都會刪除，無法復原。打字紀錄與詞庫（自訂詞、排除的詞）不受影響。";
    if !ask(window, question) {
        return;
    }
    let running = files::host_running(&app.files.host_exe);
    files::stop_host(&app.files.host_exe);
    let cleared = app.files.clear_user_model();
    if running {
        restart_engine(window, app);
    }
    match cleared {
        Ok(true) => notice(window, "個人化學習資料已清除。"),
        Ok(false) => notice(window, "目前沒有個人化學習資料。"),
        Err(err) => message(window, &format!("無法清除：{err}")),
    }
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
                    if apply(window, app) && apply_tier(window, app) {
                        unsafe {
                            let _ = DestroyWindow(window);
                        }
                    }
                }
                ID_CANCEL => unsafe {
                    let _ = DestroyWindow(window);
                },
                ID_APPLY => {
                    if apply(window, app) {
                        apply_tier(window, app);
                    }
                }
                ID_PACK_UPDATE => update_pack(window, app),
                ID_TIER_ADD => add_tier(window, app),
                ID_ROLLBACK => rollback(window, app),
                ID_REVIEW => open_review(window, app),
                ID_FOLDER => open_folder(window, app),
                ID_RESTART => restart_engine(window, app),
                ID_CLEAR => clear_personal(window, app),
                ID_PHRASES => phrases::open(window, &app.files),
                ID_DICTIONARY => dictionary::open(window, &app.files),
                _ => {}
            }
            LRESULT(0)
        }
        (models::WM_PACK_DONE, Some(app)) => {
            pack_prepared(window, app);
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
        let controls =
            INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_WIN95_CLASSES };
        let _ = InitCommonControlsEx(&controls);
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
        let app = Box::into_raw(Box::new(App { files, saved, tier: None, busy: false }));
        set_icon(window);
        build(window, &*app);
        show(window, &(*app).saved);
        show_models(window, &mut *app);
        show_status(window, &*app);
        SetWindowLongPtrW(window, GWLP_USERDATA, app as isize);
        SetTimer(Some(window), STATUS_TIMER, 2000, None);
        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetForegroundWindow(window);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // Tab, Enter and Escape as in a dialog box, in whichever of our
            // windows the message is for.
            let root = GetAncestor(msg.hwnd, GA_ROOT);
            let dialog = if root.is_invalid() { window } else { root };
            if !IsDialogMessageW(dialog, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        drop(Box::from_raw(app));
    }
}
