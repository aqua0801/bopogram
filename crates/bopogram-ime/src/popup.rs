//! The candidate window: a plain popup that never takes focus.
//!
//! It only displays. Candidates are picked with the keyboard, and all key
//! handling is in the controller.

use std::sync::Once;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreatePen, DeleteObject, EndPaint, GetDC, GetMonitorInfoW,
    GetStockObject, GetSysColor, GetSysColorBrush, GetTextExtentPoint32W, InvalidateRect,
    MonitorFromPoint, Rectangle, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TextOutW,
    CLEARTYPE_QUALITY, COLOR_GRAYTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, DEFAULT_CHARSET, HDC,
    HFONT, MONITORINFO, MONITOR_DEFAULTTONEAREST, NULL_BRUSH, PAINTSTRUCT, PS_SOLID,
    TRANSPARENT,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW, KillTimer,
    RegisterClassW, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, CS_DROPSHADOW,
    GWLP_USERDATA, HWND_TOPMOST, MA_NOACTIVATE, SWP_NOACTIVATE, SW_HIDE, WM_MOUSEACTIVATE,
    WM_NCDESTROY, WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP, SWP_SHOWWINDOW,
};

use crate::dll::module;
use crate::{guard, wide};

const CLASS_NAME: PCWSTR = w!("BopogramCandidateWindow");
const FONT_NAME: PCWSTR = w!("Microsoft JhengHei UI");
/// Text height at 96 DPI.
const FONT_PX: i32 = 18;
const PADDING_PX: i32 = 6;
const TIP_TIMER: usize = 1;
const TIP_MS: u32 = 900;

static REGISTER: Once = Once::new();

/// What the window shows: one line per entry, the last one dimmed if it is a footer.
#[derive(Default)]
struct Content {
    lines: Vec<Vec<u16>>,
    footer: bool,
}

pub struct Popup {
    window: HWND,
}

impl Popup {
    pub fn new() -> Option<Self> {
        REGISTER.call_once(|| unsafe {
            RegisterClassW(&WNDCLASSW {
                style: CS_DROPSHADOW,
                lpfnWndProc: Some(window_proc),
                hInstance: module().into(),
                hbrBackground: GetSysColorBrush(COLOR_WINDOW),
                lpszClassName: CLASS_NAME,
                ..WNDCLASSW::default()
            });
        });
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS_NAME,
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(module().into()),
                None,
            )
            .ok()?
        };
        let content = Box::into_raw(Box::<Content>::default());
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, content as isize) };
        Some(Popup { window })
    }

    /// Show `lines` just below `anchor` (screen coordinates), or above it
    /// when there is no room below.
    pub fn show(&self, lines: &[String], footer: Option<String>, anchor: RECT) {
        unsafe {
            KillTimer(Some(self.window), TIP_TIMER).ok();
            let Some(content) = content(self.window) else { return };
            content.lines = lines.iter().map(|line| wide(line)).collect();
            content.footer = footer.is_some();
            content.lines.extend(footer.map(|line| wide(&line)));

            let dpi = GetDpiForWindow(self.window).max(96) as i32;
            let padding = PADDING_PX * dpi / 96;
            let dc = GetDC(Some(self.window));
            let font = make_font(dpi);
            let previous = SelectObject(dc, font.into());
            let mut width = 0;
            let mut line_height = FONT_PX * dpi / 96;
            for line in &content.lines {
                let mut size = SIZE::default();
                let _ = GetTextExtentPoint32W(dc, line, &mut size);
                width = width.max(size.cx);
                line_height = line_height.max(size.cy);
            }
            SelectObject(dc, previous);
            let _ = DeleteObject(font.into());
            ReleaseDC(Some(self.window), dc);

            let width = width + 2 * padding;
            let height = line_height * content.lines.len() as i32 + 2 * padding;
            let mut x = anchor.left;
            let mut y = anchor.bottom + 2;
            let monitor = MonitorFromPoint(POINT { x, y: anchor.top }, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..MONITORINFO::default()
            };
            if GetMonitorInfoW(monitor, &mut info).as_bool() {
                let work = info.rcWork;
                if y + height > work.bottom {
                    y = anchor.top - height - 2;
                }
                x = x.min(work.right - width).max(work.left);
                y = y.max(work.top);
            }
            let _ = SetWindowPos(
                self.window,
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = InvalidateRect(Some(self.window), None, true);
        }
    }

    /// Show a short notice (the Chinese/English mode) that hides itself.
    pub fn show_tip(&self, text: &str, anchor: RECT) {
        self.show(&[text.to_owned()], None, anchor);
        unsafe { SetTimer(Some(self.window), TIP_TIMER, TIP_MS, None) };
    }

    pub fn hide(&self) {
        unsafe {
            KillTimer(Some(self.window), TIP_TIMER).ok();
            let _ = ShowWindow(self.window, SW_HIDE);
        }
    }
}

impl Drop for Popup {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.window);
        }
    }
}

unsafe fn content<'a>(window: HWND) -> Option<&'a mut Content> {
    unsafe { (GetWindowLongPtrW(window, GWLP_USERDATA) as *mut Content).as_mut() }
}

unsafe fn make_font(dpi: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -(FONT_PX * dpi / 96),
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
            FONT_NAME,
        )
    }
}

unsafe fn paint(window: HWND, dc: HDC) {
    unsafe {
        let Some(content) = content(window) else { return };
        let dpi = GetDpiForWindow(window).max(96) as i32;
        let padding = PADDING_PX * dpi / 96;
        let mut client = RECT::default();
        let _ = GetClientRect(window, &mut client);

        let pen = CreatePen(PS_SOLID, 1, COLORREF(GetSysColor(COLOR_GRAYTEXT)));
        let old_pen = SelectObject(dc, pen.into());
        let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        let _ = Rectangle(dc, 0, 0, client.right, client.bottom);
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        let _ = DeleteObject(pen.into());

        let font = make_font(dpi);
        let old_font = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        let count = content.lines.len().max(1) as i32;
        let line_height = (client.bottom - 2 * padding) / count;
        for (i, line) in content.lines.iter().enumerate() {
            let dimmed = content.footer && i + 1 == content.lines.len();
            let colour = if dimmed { COLOR_GRAYTEXT } else { COLOR_WINDOWTEXT };
            SetTextColor(dc, COLORREF(GetSysColor(colour)));
            let _ = TextOutW(dc, padding, padding + line_height * i as i32, line);
        }
        SelectObject(dc, old_font);
        let _ = DeleteObject(font.into());
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    guard(LRESULT(0), || unsafe {
        match message {
            WM_PAINT => {
                let mut paint_struct = PAINTSTRUCT::default();
                let dc = BeginPaint(window, &mut paint_struct);
                paint(window, dc);
                let _ = EndPaint(window, &paint_struct);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == TIP_TIMER => {
                KillTimer(Some(window), TIP_TIMER).ok();
                let _ = ShowWindow(window, SW_HIDE);
                LRESULT(0)
            }
            // A click must not take focus away from the application.
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_NCDESTROY => {
                let content = SetWindowLongPtrW(window, GWLP_USERDATA, 0) as *mut Content;
                if !content.is_null() {
                    drop(Box::from_raw(content));
                }
                DefWindowProcW(window, message, wparam, lparam)
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    })
}
