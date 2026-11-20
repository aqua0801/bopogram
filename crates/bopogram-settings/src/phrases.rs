//! The phrase editor: a window of its own, opened from the settings window.
//!
//! A list of phrases on the left; the selected one's name and text on the
//! right. Nothing is written until OK: then `phrases.json`, which the input
//! method reads each time the phrase menu opens.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetSysColorBrush, COLOR_BTNFACE};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowW,
    GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, LoadCursorW,
    RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowLongPtrW, SetWindowTextW,
    ShowWindow, GWLP_USERDATA, IDC_ARROW, SW_SHOW, WINDOW_EX_STYLE, WM_CLOSE, WM_COMMAND,
    WM_DESTROY, WNDCLASSW, WS_CAPTION, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP,
};

use crate::files::{Files, Phrase};
use crate::{item, message, ui_font, wide, Builder, ID_CANCEL, ID_OK};

const CLASS_NAME: PCWSTR = w!("BopogramPhrases");
const TITLE: &str = "片語";

/// Client area in pixels at 96 DPI.
const WIDTH: i32 = 640;
const HEIGHT: i32 = 420;

const ID_LIST: i32 = 10;
const ID_NAME: i32 = 11;
const ID_TEXT: i32 = 12;
const ID_ADD: i32 = 20;
const ID_REMOVE: i32 = 21;
const ID_UP: i32 = 22;
const ID_DOWN: i32 = 23;

// Styles, messages and notifications of list boxes and edit controls (winuser.h).
const WS_EX_CLIENTEDGE: u32 = 0x200;
const WS_VSCROLL: u32 = 0x0020_0000;
const LBS_NOTIFY: u32 = 0x1;
const LBS_NOINTEGRALHEIGHT: u32 = 0x100;
const ES_AUTOHSCROLL: u32 = 0x80;
const LB_ADDSTRING: u32 = 0x180;
const LB_INSERTSTRING: u32 = 0x181;
const LB_DELETESTRING: u32 = 0x182;
const LB_RESETCONTENT: u32 = 0x184;
const LB_SETCURSEL: u32 = 0x186;
const LB_GETCURSEL: u32 = 0x188;
const LBN_SELCHANGE: u32 = 1;
const EN_CHANGE: u32 = 0x300;
const EM_LIMITTEXT: u32 = 0xC5;

/// Longest phrase, in UTF-16 units.
const MAX_TEXT: usize = 4000;
const MAX_NAME: usize = 40;

struct Editor {
    files: Files,
    /// The settings window, disabled while this one is open.
    owner: HWND,
    phrases: Vec<Phrase>,
    current: Option<usize>,
    /// The fields are being filled in by us, not typed into.
    loading: bool,
}

/// The phrase as the list shows it.
fn label(phrase: &Phrase) -> String {
    let name = phrase.name.trim();
    let shown = if name.is_empty() { phrase.text.trim().lines().next().unwrap_or("") } else { name };
    if shown.is_empty() {
        return "（新片語）".to_owned();
    }
    let mut label: String = shown.chars().take(24).collect();
    if shown.chars().count() > 24 {
        label.push('…');
    }
    label
}

fn text_of(window: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(window).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let got = GetWindowTextW(window, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..got])
    }
}

fn set_text(window: HWND, text: &str) {
    unsafe {
        let _ = SetWindowTextW(window, PCWSTR(wide(text).as_ptr()));
    }
}

fn send(window: HWND, id: i32, msg: u32, wparam: usize, lparam: isize) -> isize {
    item(window, id).map_or(-1, |control| unsafe {
        SendMessageW(control, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))).0
    })
}

impl Editor {
    fn fill_list(&self, window: HWND) {
        send(window, ID_LIST, LB_RESETCONTENT, 0, 0);
        for phrase in &self.phrases {
            send(window, ID_LIST, LB_ADDSTRING, 0, wide(&label(phrase)).as_ptr() as isize);
        }
    }

    /// Show phrase `index` (or nothing) in the list and the fields.
    fn select(&mut self, window: HWND, index: Option<usize>) {
        self.current = index.filter(|&i| i < self.phrases.len());
        send(window, ID_LIST, LB_SETCURSEL, self.current.map_or(usize::MAX, |i| i), 0);
        let phrase = self.current.map(|i| self.phrases[i].clone()).unwrap_or_default();
        self.loading = true;
        if let Some(name) = item(window, ID_NAME) {
            set_text(name, &phrase.name);
        }
        if let Some(text) = item(window, ID_TEXT) {
            set_text(text, &phrase.text);
        }
        self.loading = false;
        let editing = self.current.is_some();
        for id in [ID_NAME, ID_TEXT, ID_REMOVE, ID_UP, ID_DOWN] {
            if let Some(control) = item(window, id) {
                unsafe {
                    let _ = EnableWindow(control, editing);
                }
            }
        }
    }

    /// A field was typed into: keep it, and show the new label.
    fn edited(&mut self, window: HWND) {
        let Some(i) = self.current.filter(|_| !self.loading) else {
            return;
        };
        let (Some(name), Some(text)) = (item(window, ID_NAME), item(window, ID_TEXT)) else {
            return;
        };
        self.phrases[i].name = text_of(name);
        self.phrases[i].text = text_of(text);
        send(window, ID_LIST, LB_DELETESTRING, i, 0);
        send(window, ID_LIST, LB_INSERTSTRING, i, wide(&label(&self.phrases[i])).as_ptr() as isize);
        send(window, ID_LIST, LB_SETCURSEL, i, 0);
    }

    fn command(&mut self, window: HWND, id: i32, code: u32) {
        match (id, code) {
            (ID_LIST, LBN_SELCHANGE) => {
                let picked = send(window, ID_LIST, LB_GETCURSEL, 0, 0);
                self.select(window, usize::try_from(picked).ok());
            }
            (ID_NAME | ID_TEXT, EN_CHANGE) => self.edited(window),
            (ID_ADD, _) => {
                self.phrases.push(Phrase::default());
                self.fill_list(window);
                self.select(window, Some(self.phrases.len() - 1));
                if let Some(text) = item(window, ID_TEXT) {
                    unsafe {
                        let _ = SetFocus(Some(text));
                    }
                }
            }
            (ID_REMOVE, _) => {
                if let Some(i) = self.current {
                    self.phrases.remove(i);
                    self.fill_list(window);
                    let next = if self.phrases.is_empty() { None } else { Some(i.min(self.phrases.len() - 1)) };
                    self.select(window, next);
                }
            }
            (ID_UP | ID_DOWN, _) => {
                if let Some(i) = self.current {
                    let j = if id == ID_UP { i.checked_sub(1) } else { Some(i + 1) };
                    if let Some(j) = j.filter(|&j| j < self.phrases.len()) {
                        self.phrases.swap(i, j);
                        self.fill_list(window);
                        self.select(window, Some(j));
                    }
                }
            }
            (ID_OK, _) => match self.files.save_phrases(&self.phrases) {
                Ok(()) => close(window, self.owner),
                Err(err) => message(window, &format!("片語無法儲存：{err}")),
            },
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
    let editor = unsafe { (GetWindowLongPtrW(window, GWLP_USERDATA) as *mut Editor).as_mut() };
    match (msg, editor) {
        (WM_COMMAND, Some(editor)) => {
            editor.command(window, (wparam.0 & 0xFFFF) as i32, ((wparam.0 >> 16) & 0xFFFF) as u32);
            LRESULT(0)
        }
        (WM_CLOSE, Some(editor)) => {
            close(window, editor.owner);
            LRESULT(0)
        }
        (WM_DESTROY, _) => {
            unsafe {
                let editor = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut Editor;
                SetWindowLongPtrW(window, GWLP_USERDATA, 0);
                if !editor.is_null() {
                    drop(Box::from_raw(editor));
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(window, msg, wparam, lparam) },
    }
}

/// Open the editor over the settings window (or bring it forward).
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
        // Over the settings window, a little in from its corner.
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
        let list = WS_TABSTOP.0 | WS_VSCROLL | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT;
        b.control_ex(WS_EX_CLIENTEDGE, w!("LISTBOX"), "", list, ID_LIST, 12, 12, 220, 352);
        b.button("新增", ID_ADD, 12, 372, 62);
        b.button("刪除", ID_REMOVE, 80, 372, 62);
        b.button("上移", ID_UP, 148, 372, 40);
        b.button("下移", ID_DOWN, 192, 372, 40);
        b.label("名稱（選填，選單中顯示；沒有時顯示內容）", -1, 248, 12, 380);
        let name = WS_TABSTOP.0 | ES_AUTOHSCROLL;
        b.control_ex(WS_EX_CLIENTEDGE, w!("EDIT"), "", name, ID_NAME, 248, 34, 380, 26);
        // One line: a line break inserted into some applications (Discord)
        // left part of it behind as an open composition.
        b.label("內容（單行）", -1, 248, 70, 380);
        let text = WS_TABSTOP.0 | ES_AUTOHSCROLL;
        b.control_ex(WS_EX_CLIENTEDGE, w!("EDIT"), "", text, ID_TEXT, 248, 92, 380, 26);
        b.button("確定", ID_OK, WIDTH - 12 - 80 * 2 - 8, 372, 80);
        b.button("取消", ID_CANCEL, WIDTH - 12 - 80, 372, 80);
        send(window, ID_NAME, EM_LIMITTEXT, MAX_NAME, 0);
        send(window, ID_TEXT, EM_LIMITTEXT, MAX_TEXT, 0);

        // Phrases written when several lines were allowed become one line,
        // shown and saved as they are now taken.
        let mut phrases = files.load_phrases();
        for phrase in &mut phrases {
            phrase.text = phrase.text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
        }
        let mut editor = Box::new(Editor { files: files.clone(), owner, phrases, current: None, loading: false });
        editor.fill_list(window);
        let first = (!editor.phrases.is_empty()).then_some(0);
        editor.select(window, first);
        SetWindowLongPtrW(window, GWLP_USERDATA, Box::into_raw(editor) as isize);

        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetForegroundWindow(window);
    }
}
