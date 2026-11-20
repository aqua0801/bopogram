//! The dictionary window: the words the user added and the text they
//! excluded, with ways to add, remove, exclude and restore. Opened from the
//! settings window. Every change goes through the engine (see `engine.rs`):
//! only the engine writes `userdict.tsv` and `excluded.tsv`.

use serde_json::{json, Value};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetSysColorBrush, COLOR_BTNFACE};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{LIST_VIEW_ITEM_FLAGS, LVCOLUMNW, LVCOLUMNW_MASK, LVITEMW, NMHDR};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowW,
    GetWindowLongPtrW, GetWindowRect, KillTimer, LoadCursorW, RegisterClassW, SendMessageW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowTextW, ShowWindow, GWLP_USERDATA,
    IDC_ARROW, SW_SHOW, WINDOW_EX_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_NOTIFY, WM_TIMER,
    WNDCLASSW, WS_CAPTION, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VSCROLL,
};

use crate::engine::{Engine, Reply};
use crate::files::Files;
use crate::{item, ui_font, wide, Builder, ID_CANCEL};

const CLASS_NAME: PCWSTR = w!("BopogramDictionary");
const TITLE: &str = "詞庫";

/// Client area in pixels at 96 DPI.
const WIDTH: i32 = 640;
const HEIGHT: i32 = 468;

const ID_FILTER: i32 = 10;
const ID_LIST: i32 = 11;
const ID_REMOVE: i32 = 12;
const ID_RESTORE: i32 = 13;
const ID_RELOAD: i32 = 14;
const ID_WORD: i32 = 20;
const ID_LOOKUP: i32 = 21;
const ID_READING: i32 = 22;
const ID_ADD: i32 = 23;
const ID_EXCLUDE: i32 = 24;
const ID_STATUS: i32 = 30;
/// Asks a starting engine again.
const RETRY_TIMER: usize = 1;

// Styles and messages of edit, combo box and list view controls (winuser.h, commctrl.h).
const WS_EX_CLIENTEDGE: u32 = 0x200;
const ES_AUTOHSCROLL: u32 = 0x80;
const CBS_DROPDOWN: u32 = 0x2;
const CBS_AUTOHSCROLL: u32 = 0x40;
const CB_ADDSTRING: u32 = 0x143;
const CB_RESETCONTENT: u32 = 0x14B;
const CB_SETCURSEL: u32 = 0x14E;
const EN_CHANGE: u32 = 0x300;
const EM_LIMITTEXT: u32 = 0xC5;
const LVS_REPORT: u32 = 0x1;
const LVS_SINGLESEL: u32 = 0x4;
const LVS_SHOWSELALWAYS: u32 = 0x8;
const LVS_EX_FULLROWSELECT: isize = 0x20;
const LVM_DELETEALLITEMS: u32 = 0x1009;
const LVM_GETNEXTITEM: u32 = 0x100C;
const LVM_SETEXTENDEDLISTVIEWSTYLE: u32 = 0x1036;
const LVM_INSERTITEMW: u32 = 0x104D;
const LVM_INSERTCOLUMNW: u32 = 0x1061;
const LVM_SETITEMTEXTW: u32 = 0x1074;
const LVNI_SELECTED: isize = 0x2;
const LVCF_WIDTH: u32 = 0x2;
const LVCF_TEXT: u32 = 0x4;
const LVIF_TEXT: u32 = 0x1;
/// LVN_FIRST - 1.
const LVN_ITEMCHANGED: u32 = (-101i32) as u32;

/// Longest word, as the engine takes it (MAX_WORD_LEN).
const MAX_WORD: usize = 8;

#[derive(Clone)]
struct Entry {
    word: String,
    /// Syllables separated by spaces; empty for an exclusion of every reading.
    reading: String,
    excluded: bool,
}

/// What a request was for, so that its answer can be shown.
#[derive(Clone)]
enum Purpose {
    List,
    Readings(String),
    Add(String, String),
    Remove(String),
    Restore(String),
    Exclude(String),
    Reload,
}

struct Window {
    engine: Engine,
    /// The settings window, disabled while this one is open.
    owner: HWND,
    entries: Vec<Entry>,
    /// The entries the list shows (the filter applied), by row.
    rows: Vec<usize>,
    /// A request the engine was not ready for, sent again by the timer.
    waiting: Option<(Value, Purpose)>,
}

fn text_of(control: Option<HWND>) -> String {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};
    let Some(control) = control else { return String::new() };
    unsafe {
        let len = GetWindowTextLengthW(control).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let got = GetWindowTextW(control, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..got]).trim().to_owned()
    }
}

fn set_text(control: Option<HWND>, text: &str) {
    if let Some(control) = control {
        unsafe {
            let _ = SetWindowTextW(control, PCWSTR(wide(text).as_ptr()));
        }
    }
}

fn send(window: HWND, id: i32, msg: u32, wparam: usize, lparam: isize) -> isize {
    item(window, id).map_or(-1, |control| unsafe {
        SendMessageW(control, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))).0
    })
}

fn enable(window: HWND, id: i32, on: bool) {
    if let Some(control) = item(window, id) {
        unsafe {
            let _ = EnableWindow(control, on);
        }
    }
}

/// Why the engine said no, in words.
fn refusal(reason: &str, error: &str) -> String {
    match reason {
        "bad_reading" => "讀音不正確：每個音節以空白分開，例如「ㄅㄛ ㄅㄛ」".to_owned(),
        "length_mismatch" => "字數和讀音的音節數不同".to_owned(),
        "too_long" => format!("太長了：最多 {MAX_WORD} 個字"),
        "duplicate" => "詞庫裡已經有這個詞和讀音".to_owned(),
        "not_found" => "詞庫裡沒有這個詞".to_owned(),
        "last_candidate" => "這是這個讀音唯一的字，不能排除".to_owned(),
        "disabled" => "詞庫功能已關閉（config.json 的 user_dict 或 excluded_words 設成 null）".to_owned(),
        "write_failed" => "詞庫檔案寫不進去，沒有變更".to_owned(),
        "restricted" => "這個程式不能修改詞庫".to_owned(),
        _ if error.is_empty() => "引擎拒絕了這個操作".to_owned(),
        _ => format!("引擎拒絕了這個操作：{error}"),
    }
}

impl Window {
    fn status(&self, window: HWND, text: &str) {
        set_text(item(window, ID_STATUS), text);
    }

    /// Ask the engine; a starting engine is asked again every second.
    fn ask(&mut self, window: HWND, request: Value, purpose: Purpose) {
        match self.engine.request(request.clone()) {
            Reply::Done(value) => self.done(window, &value, purpose),
            Reply::Refused(reason, error) => {
                let text = match (&purpose, reason.as_str()) {
                    (Purpose::Add(word, _), "duplicate") => format!("詞庫裡已經有「{word}」"),
                    (Purpose::Readings(_), "too_long") => format!("太長了：最多 {MAX_WORD} 個字"),
                    _ => refusal(&reason, &error),
                };
                self.status(window, &text);
            }
            Reply::Starting => {
                self.waiting = Some((request, purpose));
                self.status(window, "引擎啟動中，請稍候…");
                unsafe { SetTimer(Some(window), RETRY_TIMER, 1000, None) };
            }
            Reply::Unreachable(why) => self.status(window, &format!("連不上引擎：{why}")),
        }
    }

    fn retry(&mut self, window: HWND) {
        unsafe {
            let _ = KillTimer(Some(window), RETRY_TIMER);
        }
        if let Some((request, purpose)) = self.waiting.take() {
            self.ask(window, request, purpose);
        }
    }

    fn done(&mut self, window: HWND, value: &Value, purpose: Purpose) {
        let changed = match purpose {
            Purpose::List => {
                self.entries = value
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|entry| {
                        let field = |name: &str| entry.get(name).and_then(Value::as_str).map(str::to_owned);
                        Some(Entry {
                            word: field("word")?,
                            reading: field("reading").unwrap_or_default(),
                            excluded: field("kind").as_deref() == Some("excluded"),
                        })
                    })
                    .collect();
                self.fill(window);
                let added = self.entries.iter().filter(|e| !e.excluded).count();
                let excluded = self.entries.len() - added;
                self.status(window, &format!("自訂詞 {added} 個，排除 {excluded} 個"));
                return;
            }
            Purpose::Readings(word) => {
                let readings: Vec<(String, bool)> = value
                    .get("readings")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|r| {
                        let known = r.get("known").and_then(Value::as_bool).unwrap_or(false);
                        Some((r.get("reading")?.as_str()?.to_owned(), known))
                    })
                    .collect();
                send(window, ID_READING, CB_RESETCONTENT, 0, 0);
                for (reading, _) in &readings {
                    send(window, ID_READING, CB_ADDSTRING, 0, wide(reading).as_ptr() as isize);
                }
                send(window, ID_READING, CB_SETCURSEL, 0, 0);
                let text = match readings.as_slice() {
                    [] => format!("查不到「{word}」的讀音：請在「讀音」自己輸入注音"),
                    [(_, true)] => format!("「{word}」已經在字典或詞庫裡"),
                    [_] => "找到 1 個讀音".to_owned(),
                    many => format!("找到 {} 個讀音，請選出正確的一個", many.len()),
                };
                self.status(window, &text);
                return;
            }
            Purpose::Add(word, reading) => format!("已加入：{word}（{reading}）"),
            Purpose::Remove(word) => match value.get("removed").and_then(Value::as_str) {
                Some("excluded") => format!("已刪除「{word}」；它也在字典裡，所以同時排除"),
                _ => format!("已刪除「{word}」"),
            },
            Purpose::Restore(word) => format!("已取消排除「{word}」"),
            Purpose::Exclude(text) => format!("已排除「{text}」：不會再被自動選出"),
            Purpose::Reload => "已重新讀取詞庫檔案".to_owned(),
        };
        self.ask(window, json!({"op": "dict_list"}), Purpose::List);
        self.status(window, &changed);
    }

    /// Show the entries that match the filter.
    fn fill(&mut self, window: HWND) {
        let filter = text_of(item(window, ID_FILTER));
        send(window, ID_LIST, LVM_DELETEALLITEMS, 0, 0);
        self.rows = (0..self.entries.len())
            .filter(|&i| {
                let entry = &self.entries[i];
                filter.is_empty() || entry.word.contains(&filter) || entry.reading.contains(&filter)
            })
            .collect();
        for (row, &i) in self.rows.iter().enumerate() {
            let entry = &self.entries[i];
            let reading = if entry.reading.is_empty() { "（全部讀音）" } else { &entry.reading };
            let kind = if entry.excluded { "已排除" } else { "自訂" };
            for (column, text) in [&entry.word as &str, reading, kind].into_iter().enumerate() {
                let mut text = wide(text);
                let cell = LVITEMW {
                    mask: LIST_VIEW_ITEM_FLAGS(LVIF_TEXT),
                    iItem: row as i32,
                    iSubItem: column as i32,
                    pszText: PWSTR(text.as_mut_ptr()),
                    ..Default::default()
                };
                let msg = if column == 0 { LVM_INSERTITEMW } else { LVM_SETITEMTEXTW };
                send(window, ID_LIST, msg, row, &cell as *const _ as isize);
            }
        }
        self.selection_changed(window);
    }

    fn selected(&self, window: HWND) -> Option<Entry> {
        let row = send(window, ID_LIST, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED);
        let i = *self.rows.get(usize::try_from(row).ok()?)?;
        self.entries.get(i).cloned()
    }

    fn selection_changed(&self, window: HWND) {
        let selected = self.selected(window);
        enable(window, ID_REMOVE, selected.as_ref().is_some_and(|e| !e.excluded));
        enable(window, ID_RESTORE, selected.as_ref().is_some_and(|e| e.excluded));
    }

    fn command(&mut self, window: HWND, id: i32, code: u32) {
        let word = || text_of(item(window, ID_WORD));
        match (id, code) {
            (ID_FILTER, EN_CHANGE) => self.fill(window),
            (ID_REMOVE, _) => {
                if let Some(entry) = self.selected(window) {
                    let request = json!({"op": "dict_remove", "word": entry.word, "reading": entry.reading});
                    self.ask(window, request, Purpose::Remove(entry.word));
                }
            }
            (ID_RESTORE, _) => {
                if let Some(entry) = self.selected(window) {
                    let mut request = json!({"op": "dict_restore", "word": entry.word});
                    if !entry.reading.is_empty() {
                        request["reading"] = entry.reading.into();
                    }
                    self.ask(window, request, Purpose::Restore(entry.word));
                }
            }
            (ID_RELOAD, _) => self.ask(window, json!({"op": "dict_reload"}), Purpose::Reload),
            (ID_LOOKUP, _) => {
                let word = word();
                if word.is_empty() {
                    self.status(window, "請先在「詞」輸入要加入的詞");
                    return;
                }
                self.ask(window, json!({"op": "readings", "text": word}), Purpose::Readings(word));
            }
            (ID_ADD, _) => {
                let (word, reading) = (word(), text_of(item(window, ID_READING)));
                if word.is_empty() || reading.is_empty() {
                    self.status(window, "請輸入詞，按「查讀音」選出讀音後再加入");
                    return;
                }
                let request = json!({"op": "dict_add", "word": word, "reading": reading});
                self.ask(window, request, Purpose::Add(word, reading));
            }
            (ID_EXCLUDE, _) => {
                let text = word();
                if text.is_empty() {
                    self.status(window, "請先在「詞」輸入要排除的文字");
                    return;
                }
                self.ask(window, json!({"op": "dict_exclude", "text": text}), Purpose::Exclude(text));
            }
            (ID_CANCEL, _) => close(window, self.owner),
            _ => {}
        }
    }
}

fn close(window: HWND, owner: HWND) {
    unsafe {
        let _ = EnableWindow(owner, true);
        let _ = DestroyWindow(window);
        let _ = SetForegroundWindow(owner);
    }
}

unsafe extern "system" fn window_proc(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let state = unsafe { (GetWindowLongPtrW(window, GWLP_USERDATA) as *mut Window).as_mut() };
    match (msg, state) {
        (WM_COMMAND, Some(state)) => {
            state.command(window, (wparam.0 & 0xFFFF) as i32, ((wparam.0 >> 16) & 0xFFFF) as u32);
            LRESULT(0)
        }
        (WM_NOTIFY, Some(state)) => {
            let header = unsafe { &*(lparam.0 as *const NMHDR) };
            if header.idFrom == ID_LIST as usize && header.code == LVN_ITEMCHANGED {
                state.selection_changed(window);
            }
            LRESULT(0)
        }
        (WM_TIMER, Some(state)) => {
            state.retry(window);
            LRESULT(0)
        }
        (WM_CLOSE, Some(state)) => {
            close(window, state.owner);
            LRESULT(0)
        }
        (WM_DESTROY, _) => {
            unsafe {
                let _ = KillTimer(Some(window), RETRY_TIMER);
                let state = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut Window;
                SetWindowLongPtrW(window, GWLP_USERDATA, 0);
                if !state.is_null() {
                    drop(Box::from_raw(state));
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(window, msg, wparam, lparam) },
    }
}

/// Open the dictionary window over the settings window (or bring it forward).
pub fn open(owner: HWND, files: &Files) {
    unsafe {
        if let Ok(open) = FindWindowW(CLASS_NAME, PCWSTR::null()) {
            let _ = SetForegroundWindow(open);
            return;
        }
        let Ok(instance) = GetModuleHandleW(None) else { return };
        // Fails harmlessly when the class is already registered.
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            lpszClassName: CLASS_NAME,
            ..WNDCLASSW::default()
        });
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let dpi = GetDpiForSystem() as i32;
        let mut frame = RECT { left: 0, top: 0, right: WIDTH * dpi / 96, bottom: HEIGHT * dpi / 96 };
        let _ = AdjustWindowRectEx(&mut frame, style, false, WINDOW_EX_STYLE(0));
        let mut at = RECT::default();
        let _ = GetWindowRect(owner, &mut at);
        let Ok(window) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            PCWSTR(wide(TITLE).as_ptr()),
            style,
            at.left + 40 * dpi / 96,
            at.top + 40 * dpi / 96,
            frame.right - frame.left,
            frame.bottom - frame.top,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };
        crate::set_icon(window);

        let b = Builder { window, font: ui_font(dpi), dpi, tooltip: None };
        b.label("搜尋", -1, 12, 16, 44);
        let edit = WS_TABSTOP.0 | ES_AUTOHSCROLL;
        b.control_ex(WS_EX_CLIENTEDGE, w!("EDIT"), "", edit, ID_FILTER, 60, 12, 260, 26);
        b.button("重新讀取檔案", ID_RELOAD, WIDTH - 12 - 120, 11, 120);

        let list = WS_TABSTOP.0 | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS;
        b.control_ex(WS_EX_CLIENTEDGE, w!("SysListView32"), "", list, ID_LIST, 12, 46, WIDTH - 24, 220);
        send(window, ID_LIST, LVM_SETEXTENDEDLISTVIEWSTYLE, 0, LVS_EX_FULLROWSELECT);
        for (i, (name, width)) in [("詞", 200), ("讀音", 260), ("類型", 120)].into_iter().enumerate() {
            let mut name = wide(name);
            let column = LVCOLUMNW {
                mask: LVCOLUMNW_MASK(LVCF_TEXT | LVCF_WIDTH),
                cx: width * dpi / 96,
                pszText: PWSTR(name.as_mut_ptr()),
                ..Default::default()
            };
            send(window, ID_LIST, LVM_INSERTCOLUMNW, i, &column as *const _ as isize);
        }
        b.button("刪除自訂詞", ID_REMOVE, 12, 274, 120);
        b.button("取消排除", ID_RESTORE, 140, 274, 120);

        b.label("加入或排除", -1, 12, 318, 300);
        b.label("詞", -1, 12, 346, 44);
        b.control_ex(WS_EX_CLIENTEDGE, w!("EDIT"), "", edit, ID_WORD, 60, 342, 200, 26);
        send(window, ID_WORD, EM_LIMITTEXT, MAX_WORD * 2, 0);
        b.button("查讀音", ID_LOOKUP, 268, 341, 90);
        b.button("排除這段文字", ID_EXCLUDE, 366, 341, 130);
        b.label("讀音", -1, 12, 382, 44);
        let combo = WS_TABSTOP.0 | WS_VSCROLL.0 | CBS_DROPDOWN | CBS_AUTOHSCROLL;
        b.control(w!("COMBOBOX"), "", combo, ID_READING, 60, 378, 298, 200);
        b.button("加入詞庫", ID_ADD, 366, 377, 130);

        b.label("", ID_STATUS, 12, 428, WIDTH - 24 - 92);
        b.button("關閉", ID_CANCEL, WIDTH - 12 - 80, 422, 80);

        let state = Box::new(Window {
            engine: Engine::new(files.host_exe.clone()),
            owner,
            entries: Vec::new(),
            rows: Vec::new(),
            waiting: None,
        });
        let state = Box::into_raw(state);
        SetWindowLongPtrW(window, GWLP_USERDATA, state as isize);
        (*state).selection_changed(window);
        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(window, SW_SHOW);
        (*state).ask(window, json!({"op": "dict_list"}), Purpose::List);
    }
}
