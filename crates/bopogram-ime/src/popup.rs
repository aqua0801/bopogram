//! The candidate window: a popup that never takes focus.
//!
//! It shows the current page of candidates (numbered 1-9), the highlighted
//! one marked, and the page number when there is more than one page. The
//! mouse can pick a candidate, turn pages with the wheel or the arrows at
//! the bottom; what that means is decided by the text service, which hears
//! about it through the callback given to `Popup::new`.
//!
//! It follows the light or dark app theme, takes the text size from the
//! settings, and is drawn for the DPI of the monitor it appears on.
//!
//! The same window also shows short notices (the Chinese/English mode).

use std::rc::Rc;
use std::sync::Once;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush,
    DeleteDC, DeleteObject, EndPaint, FillRect, GetDC, GetMonitorInfoW, GetSysColor,
    GetTextExtentPoint32W, InvalidateRect, MonitorFromPoint, ReleaseDC, SelectObject, SetBkMode,
    SetTextColor, TextOutW, CLEARTYPE_QUALITY, COLOR_HIGHLIGHT, DEFAULT_CHARSET, HDC, HFONT,
    HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::HiDpi::{
    GetAwarenessFromDpiAwarenessContext, GetDpiForMonitor, GetDpiForWindow,
    GetThreadDpiAwarenessContext, DPI_AWARENESS_PER_MONITOR_AWARE, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW, KillTimer,
    RegisterClassW, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, CS_DROPSHADOW,
    GWLP_USERDATA, HWND_TOPMOST, MA_NOACTIVATE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE,
    WM_ERASEBKGND, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NCDESTROY, WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use crate::dll::module;
use crate::{guard, settings, wide};

/// winuser.h; windows-rs files it under the common controls.
const WM_MOUSELEAVE: u32 = 0x02A3;

const CLASS_NAME: PCWSTR = w!("BopogramCandidateWindow");
const FONT_NAME: PCWSTR = w!("Microsoft JhengHei UI");
/// Sizes at 96 DPI.
const PADDING_PX: i32 = 4;
const ROW_GAP_PX: i32 = 4;
const NUMBER_GAP_PX: i32 = 8;
const MIN_WIDTH_PX: i32 = 96;
const TIP_TIMER: usize = 1;
const TIP_MS: u32 = 900;

static REGISTER: Once = Once::new();

/// What the mouse did, for the text service to act on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// A candidate was clicked: its position on the shown page.
    Pick(usize),
    /// Show the next (1) or previous (-1) page.
    Page(isize),
}

pub type Handler = Rc<dyn Fn(Event)>;

#[derive(Clone, Copy)]
struct Palette {
    background: COLORREF,
    text: COLORREF,
    dim: COLORREF,
    border: COLORREF,
    hover: COLORREF,
    selected: COLORREF,
    selected_text: COLORREF,
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

impl Palette {
    fn current() -> Self {
        let selected = COLORREF(unsafe { GetSysColor(COLOR_HIGHLIGHT) });
        if dark_theme() {
            Palette {
                background: rgb(0x2b, 0x2b, 0x2b),
                text: rgb(0xf2, 0xf2, 0xf2),
                dim: rgb(0xa0, 0xa0, 0xa0),
                border: rgb(0x55, 0x55, 0x55),
                hover: rgb(0x3d, 0x3d, 0x3d),
                selected,
                selected_text: rgb(0xff, 0xff, 0xff),
            }
        } else {
            Palette {
                background: rgb(0xff, 0xff, 0xff),
                text: rgb(0x1a, 0x1a, 0x1a),
                dim: rgb(0x70, 0x70, 0x70),
                border: rgb(0xc4, 0xc4, 0xc4),
                hover: rgb(0xee, 0xee, 0xee),
                selected,
                selected_text: rgb(0xff, 0xff, 0xff),
            }
        }
    }
}

/// Does the user use the dark theme for applications?
fn dark_theme() -> bool {
    is_dark(w!("AppsUseLightTheme"))
}

/// Is this theme setting (`AppsUseLightTheme`, `SystemUsesLightTheme`) dark?
pub(crate) fn is_dark(setting: PCWSTR) -> bool {
    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            setting,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    read.is_ok() && value == 0
}

enum Mode {
    Tip(Vec<u16>),
    /// `labels`: the selection key of each row.
    List { words: Vec<Vec<u16>>, labels: Vec<Vec<u16>>, selected: usize, page: usize, pages: usize },
}

/// Where things were drawn, for finding what the mouse is on.
#[derive(Default)]
struct Layout {
    rows: Vec<RECT>,
    previous: Option<RECT>,
    next: Option<RECT>,
}

struct Content {
    mode: Mode,
    dpi: i32,
    font_px: i32,
    palette: Palette,
    hover: Option<usize>,
    tracking: bool,
    layout: Layout,
    handler: Option<Handler>,
}

pub struct Popup {
    window: HWND,
}

fn scale(value: i32, dpi: i32) -> i32 {
    value * dpi / 96
}

fn contains(rect: &RECT, x: i32, y: i32) -> bool {
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

/// The DPI to draw at near `point`. A window in a per-monitor aware thread
/// gets the monitor's own; otherwise Windows scales the window itself and
/// the thread's DPI is the one that counts.
fn dpi_at(window: HWND, monitor: HMONITOR) -> i32 {
    unsafe {
        let aware = GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext())
            == DPI_AWARENESS_PER_MONITOR_AWARE;
        if aware {
            let (mut x, mut y) = (0u32, 0u32);
            if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y).is_ok() && x >= 96 {
                return x as i32;
            }
        }
        GetDpiForWindow(window).max(96) as i32
    }
}

unsafe fn make_font(px: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -px,
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

/// Room for the widest selection key.
fn label_width(dc: HDC, labels: &[Vec<u16>]) -> i32 {
    labels.iter().map(|label| text_size(dc, label).cx).max().unwrap_or(0)
}

fn text_size(dc: HDC, text: &[u16]) -> SIZE {
    let mut size = SIZE::default();
    unsafe {
        let _ = GetTextExtentPoint32W(dc, text, &mut size);
    }
    size
}

impl Content {
    fn footer(&self) -> Option<Vec<u16>> {
        match &self.mode {
            Mode::List { page, pages, .. } if *pages > 1 => {
                Some(wide(&format!("{}/{}", page + 1, pages)))
            }
            _ => None,
        }
    }

    /// Work out where everything goes; returns the window size.
    fn measure(&mut self, dc: HDC) -> SIZE {
        let dpi = self.dpi;
        let padding = scale(PADDING_PX, dpi);
        let gap = scale(ROW_GAP_PX, dpi);
        let line = text_size(dc, &wide("中")).cy;
        let row_height = line + gap;
        let mut layout = Layout::default();
        let size = match &self.mode {
            Mode::Tip(text) => {
                let width = text_size(dc, text).cx + 2 * padding + 2 * gap;
                SIZE { cx: width, cy: row_height + 2 * padding }
            }
            Mode::List { words, labels, .. } => {
                let number = label_width(dc, labels) + scale(NUMBER_GAP_PX, dpi);
                let widest = words.iter().map(|word| text_size(dc, word).cx).max().unwrap_or(0);
                let mut width = (number + widest + 2 * gap).max(scale(MIN_WIDTH_PX, dpi));
                let footer = self.footer();
                if let Some(footer) = &footer {
                    width = width.max(text_size(dc, footer).cx + 4 * line);
                }
                let mut top = padding;
                for _ in words {
                    layout.rows.push(RECT {
                        left: padding,
                        top,
                        right: padding + width,
                        bottom: top + row_height,
                    });
                    top += row_height;
                }
                if footer.is_some() {
                    let third = width / 3;
                    layout.previous = Some(RECT {
                        left: padding,
                        top,
                        right: padding + third,
                        bottom: top + row_height,
                    });
                    layout.next = Some(RECT {
                        left: padding + width - third,
                        top,
                        right: padding + width,
                        bottom: top + row_height,
                    });
                    top += row_height;
                }
                SIZE { cx: width + 2 * padding, cy: top + padding }
            }
        };
        self.layout = layout;
        size
    }

    fn paint(&self, dc: HDC, client: RECT) {
        let palette = self.palette;
        let dpi = self.dpi;
        let gap = scale(ROW_GAP_PX, dpi);
        unsafe {
            fill(dc, client, palette.border);
            let inner = RECT { left: 1, top: 1, right: client.right - 1, bottom: client.bottom - 1 };
            fill(dc, inner, palette.background);
            let font = make_font(self.font_px);
            let old_font = SelectObject(dc, font.into());
            SetBkMode(dc, TRANSPARENT);
            let line = text_size(dc, &wide("中")).cy;
            match &self.mode {
                Mode::Tip(text) => {
                    let width = text_size(dc, text).cx;
                    SetTextColor(dc, palette.text);
                    let _ = TextOutW(
                        dc,
                        (client.right - width) / 2,
                        (client.bottom - line) / 2,
                        text,
                    );
                }
                Mode::List { words, labels, selected, .. } => {
                    let number_width = label_width(dc, labels) + scale(NUMBER_GAP_PX, dpi);
                    for (i, (word, row)) in words.iter().zip(&self.layout.rows).enumerate() {
                        let (number_colour, word_colour) = if i == *selected {
                            fill(dc, *row, palette.selected);
                            (palette.selected_text, palette.selected_text)
                        } else {
                            if self.hover == Some(i) {
                                fill(dc, *row, palette.hover);
                            }
                            (palette.dim, palette.text)
                        };
                        let y = row.top + gap / 2;
                        SetTextColor(dc, number_colour);
                        if let Some(label) = labels.get(i) {
                            let _ = TextOutW(dc, row.left + gap, y, label);
                        }
                        SetTextColor(dc, word_colour);
                        let _ = TextOutW(dc, row.left + gap + number_width, y, word);
                    }
                    if let (Some(footer), Some(previous), Some(next)) =
                        (self.footer(), self.layout.previous, self.layout.next)
                    {
                        let hover_previous = self.hover == Some(usize::MAX - 1);
                        let hover_next = self.hover == Some(usize::MAX);
                        if hover_previous {
                            fill(dc, previous, palette.hover);
                        }
                        if hover_next {
                            fill(dc, next, palette.hover);
                        }
                        SetTextColor(dc, palette.dim);
                        let y = previous.top + gap / 2;
                        let left = wide("‹");
                        let right = wide("›");
                        let _ = TextOutW(dc, previous.left + gap, y, &left);
                        let _ = TextOutW(dc, next.right - gap - text_size(dc, &right).cx, y, &right);
                        let width = text_size(dc, &footer).cx;
                        let _ = TextOutW(dc, (client.right - width) / 2, y, &footer);
                    }
                }
            }
            SelectObject(dc, old_font);
            let _ = DeleteObject(font.into());
        }
    }

    /// What is under the mouse: a row, or the previous/next arrows
    /// (`usize::MAX - 1`, `usize::MAX`).
    fn hit(&self, x: i32, y: i32) -> Option<usize> {
        if let Some(row) = self.layout.rows.iter().position(|row| contains(row, x, y)) {
            return Some(row);
        }
        if self.layout.previous.is_some_and(|r| contains(&r, x, y)) {
            return Some(usize::MAX - 1);
        }
        if self.layout.next.is_some_and(|r| contains(&r, x, y)) {
            return Some(usize::MAX);
        }
        None
    }
}

fn fill(dc: HDC, rect: RECT, colour: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(colour);
        FillRect(dc, &rect, brush);
        let _ = DeleteObject(brush.into());
    }
}

impl Popup {
    /// `handler` hears about clicks and the wheel.
    pub fn new(handler: Option<Handler>) -> Option<Self> {
        REGISTER.call_once(|| unsafe {
            RegisterClassW(&WNDCLASSW {
                style: CS_DROPSHADOW,
                lpfnWndProc: Some(window_proc),
                hInstance: module().into(),
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
        let content = Box::into_raw(Box::new(Content {
            mode: Mode::Tip(Vec::new()),
            dpi: 96,
            font_px: settings::candidate_font_px(),
            palette: Palette::current(),
            hover: None,
            tracking: false,
            layout: Layout::default(),
            handler,
        }));
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, content as _) };
        Some(Popup { window })
    }

    /// Show a page of candidates just below `anchor` (screen coordinates),
    /// or above it when there is no room below.
    /// `labels` are the selection keys, shown upper case.
    pub fn show(
        &self,
        words: &[String],
        labels: &[char],
        selected: usize,
        page: usize,
        pages: usize,
        anchor: RECT,
    ) {
        let words = words.iter().map(|word| wide(word)).collect();
        let labels = labels.iter().map(|key| wide(&key.to_ascii_uppercase().to_string())).collect();
        self.place(Mode::List { words, labels, selected, page, pages }, anchor);
    }

    /// Show a short notice (the Chinese/English mode) that hides itself.
    pub fn show_tip(&self, text: &str, anchor: RECT) {
        self.place(Mode::Tip(wide(text)), anchor);
        unsafe { SetTimer(Some(self.window), TIP_TIMER, TIP_MS, None) };
    }

    fn place(&self, mode: Mode, anchor: RECT) {
        unsafe {
            KillTimer(Some(self.window), TIP_TIMER).ok();
            let Some(content) = content(self.window) else { return };
            let tip = matches!(mode, Mode::Tip(_));
            let monitor = MonitorFromPoint(POINT { x: anchor.left, y: anchor.top }, MONITOR_DEFAULTTONEAREST);
            content.mode = mode;
            content.dpi = dpi_at(self.window, monitor);
            content.font_px = scale(settings::candidate_font_px(), content.dpi);
            content.palette = Palette::current();
            if tip {
                content.hover = None;
            }

            let dc = GetDC(Some(self.window));
            let font = make_font(content.font_px);
            let previous = SelectObject(dc, font.into());
            let size = content.measure(dc);
            SelectObject(dc, previous);
            let _ = DeleteObject(font.into());
            ReleaseDC(Some(self.window), dc);

            let (width, height) = (size.cx, size.cy);
            let mut x = anchor.left;
            let mut y = anchor.bottom + 2;
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
                y = y.min(work.bottom - height).max(work.top);
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
            let _ = InvalidateRect(Some(self.window), None, false);
        }
    }

    pub fn hide(&self) {
        unsafe {
            KillTimer(Some(self.window), TIP_TIMER).ok();
            if let Some(content) = content(self.window) {
                content.hover = None;
            }
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

/// Paint into an off-screen bitmap first, so the window does not flicker.
unsafe fn paint(window: HWND, dc: HDC) {
    unsafe {
        let Some(content) = content(window) else { return };
        let mut client = RECT::default();
        let _ = GetClientRect(window, &mut client);
        let memory = CreateCompatibleDC(Some(dc));
        let bitmap = CreateCompatibleBitmap(dc, client.right, client.bottom);
        let old = SelectObject(memory, bitmap.into());
        content.paint(memory, client);
        let _ = BitBlt(dc, 0, 0, client.right, client.bottom, Some(memory), 0, 0, SRCCOPY);
        SelectObject(memory, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
    }
}

fn point(lparam: LPARAM) -> (i32, i32) {
    ((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32)
}

/// Tell the text service, with nothing of ours borrowed: it redraws this
/// window in response.
fn notify(window: HWND, event: Event) {
    let handler = unsafe { content(window) }.and_then(|c| c.handler.clone());
    if let Some(handler) = handler {
        handler(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{
        GetDIBits, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    };

    /// Draws the window as it would look, into a bitmap file to look at:
    /// `cargo test -p bopogram-ime --lib popup -- --ignored`, then open
    /// `%TEMP%\bopogram-popup-*.bmp`.
    #[test]
    #[ignore = "writes pictures for a person to look at"]
    fn draw_to_files() {
        let palettes = [("light", false), ("dark", true)];
        for (name, dark) in palettes {
            let mut content = Content {
                mode: Mode::List {
                    words: ["道", "到", "倒", "導", "盜", "悼", "稻", "蹈", "纛"].iter().map(|w| wide(w)).collect(),
                    labels: "123456789".chars().map(|c| wide(&c.to_string())).collect(),
                    selected: 2,
                    page: 0,
                    pages: 3,
                },
                dpi: 144,
                font_px: scale(DEFAULT_FONT, 144),
                palette: Palette::current(),
                hover: Some(5),
                tracking: false,
                layout: Layout::default(),
                handler: None,
            };
            let selected = content.palette.selected;
            content.palette = if dark {
                Palette {
                    background: rgb(0x2b, 0x2b, 0x2b),
                    text: rgb(0xf2, 0xf2, 0xf2),
                    dim: rgb(0xa0, 0xa0, 0xa0),
                    border: rgb(0x55, 0x55, 0x55),
                    hover: rgb(0x3d, 0x3d, 0x3d),
                    selected,
                    selected_text: rgb(0xff, 0xff, 0xff),
                }
            } else {
                Palette {
                    background: rgb(0xff, 0xff, 0xff),
                    text: rgb(0x1a, 0x1a, 0x1a),
                    dim: rgb(0x70, 0x70, 0x70),
                    border: rgb(0xc4, 0xc4, 0xc4),
                    hover: rgb(0xee, 0xee, 0xee),
                    selected,
                    selected_text: rgb(0xff, 0xff, 0xff),
                }
            };
            unsafe {
                let screen = GetDC(None);
                let dc = CreateCompatibleDC(Some(screen));
                let font = make_font(content.font_px);
                let old_font = SelectObject(dc, font.into());
                let size = content.measure(dc);
                SelectObject(dc, old_font);
                let _ = DeleteObject(font.into());
                let bitmap = CreateCompatibleBitmap(screen, size.cx, size.cy);
                let old = SelectObject(dc, bitmap.into());
                content.paint(dc, RECT { left: 0, top: 0, right: size.cx, bottom: size.cy });
                SelectObject(dc, old);

                let mut info = BITMAPINFO::default();
                info.bmiHeader = BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size.cx,
                    biHeight: size.cy,
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                };
                let mut pixels = vec![0u8; (size.cx * size.cy * 4) as usize];
                GetDIBits(dc, bitmap, 0, size.cy as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(dc);
                ReleaseDC(None, screen);

                let mut file = Vec::new();
                let header = 14 + 40;
                file.extend_from_slice(b"BM");
                file.extend_from_slice(&((header + pixels.len()) as u32).to_le_bytes());
                file.extend_from_slice(&0u32.to_le_bytes());
                file.extend_from_slice(&(header as u32).to_le_bytes());
                file.extend_from_slice(&40u32.to_le_bytes());
                file.extend_from_slice(&size.cx.to_le_bytes());
                file.extend_from_slice(&size.cy.to_le_bytes());
                file.extend_from_slice(&1u16.to_le_bytes());
                file.extend_from_slice(&32u16.to_le_bytes());
                file.extend_from_slice(&[0u8; 24]);
                file.extend_from_slice(&pixels);
                let path = std::env::temp_dir().join(format!("bopogram-popup-{name}.bmp"));
                std::fs::write(&path, file).unwrap();
                println!("{}", path.display());
            }
        }
    }

    const DEFAULT_FONT: i32 = settings::DEFAULT_FONT_PX;
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
            WM_ERASEBKGND => LRESULT(1),
            WM_TIMER if wparam.0 == TIP_TIMER => {
                KillTimer(Some(window), TIP_TIMER).ok();
                let _ = ShowWindow(window, SW_HIDE);
                LRESULT(0)
            }
            // A click must not take focus away from the application.
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_MOUSEMOVE => {
                if let Some(content) = content(window) {
                    if !content.tracking {
                        let mut track = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: window,
                            dwHoverTime: 0,
                        };
                        content.tracking = TrackMouseEvent(&mut track).is_ok();
                    }
                    let (x, y) = point(lparam);
                    let hover = content.hit(x, y);
                    if hover != content.hover && matches!(content.mode, Mode::List { .. }) {
                        content.hover = hover;
                        let _ = InvalidateRect(Some(window), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                if let Some(content) = content(window) {
                    content.tracking = false;
                    if content.hover.take().is_some() {
                        let _ = InvalidateRect(Some(window), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = point(lparam);
                let hit = content(window)
                    .filter(|c| matches!(c.mode, Mode::List { .. }))
                    .and_then(|c| c.hit(x, y));
                match hit {
                    Some(usize::MAX) => notify(window, Event::Page(1)),
                    Some(i) if i == usize::MAX - 1 => notify(window, Event::Page(-1)),
                    Some(row) => notify(window, Event::Pick(row)),
                    None => {}
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam.0 >> 16) & 0xffff) as i16;
                if delta != 0 {
                    notify(window, Event::Page(if delta < 0 { 1 } else { -1 }));
                }
                LRESULT(0)
            }
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
