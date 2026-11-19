//! The mode button: the icon Windows shows in the taskbar's input indicator
//! while this input method is selected.
//!
//! Left click switches Chinese/English. Right click opens a small menu:
//! the settings window and the casual/formal style. The settings window is a
//! separate program (`bopogram-settings.exe`); nothing of it lives here.
//!
//! Windows may ask for the menu in two ways, depending on its version: it
//! either reports the right click (`OnClick`), and we show a popup menu, or
//! it builds the menu itself (`InitMenu` / `OnMenuSelect`). Both are served.

use windows::core::{implement, w, Interface, Ref, Result, BOOL, BSTR, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    COLORREF, E_FAIL, E_INVALIDARG, HWND, POINT, RECT,
};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush,
    DeleteDC, DeleteObject, DrawTextW, FillRect, SelectObject, SetBkMode, SetTextColor,
    BITMAPINFO, BITMAPINFOHEADER, CLEARTYPE_QUALITY, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CENTER,
    DT_NOCLIP, DT_SINGLELINE, DT_VCENTER, TRANSPARENT,
};
use windows::Win32::UI::TextServices::{
    ITfLangBarItem, ITfLangBarItemButton, ITfLangBarItemButton_Impl, ITfLangBarItemSink,
    ITfLangBarItem_Impl, ITfMenu, ITfSource, ITfSource_Impl, TfLBIClick, GUID_LBI_INPUTMODE,
    TF_LANGBARITEMINFO, TF_LBI_CLK_LEFT, TF_LBI_CLK_RIGHT, TF_LBI_STYLE_BTN_BUTTON,
    TF_LBI_STYLE_SHOWNINTRAY, TF_LBMENUF_CHECKED, TF_LBMENUF_SEPARATOR,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DestroyMenu,
    DestroyWindow, GetSystemMetrics, SetForegroundWindow, TrackPopupMenu, HICON, ICONINFO,
    MF_CHECKED, MF_SEPARATOR, MF_STRING, SM_CXSMICON, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::dll::{module, CLSID_TEXT_SERVICE};
use crate::service::{self, Shared};
use crate::{guard, wide};

const MENU_SETTINGS: u32 = 1;
const MENU_CASUAL: u32 = 2;
const MENU_FORMAL: u32 = 3;
const SINK_COOKIE: u32 = 1;

// Connection point errors (olectl.h).
const CONNECT_E_NOCONNECTION: HRESULT = HRESULT(0x8004_0200_u32 as i32);
const CONNECT_E_ADVISELIMIT: HRESULT = HRESULT(0x8004_0201_u32 as i32);
const CONNECT_E_CANNOTCONNECT: HRESULT = HRESULT(0x8004_0202_u32 as i32);

const TEXT_SETTINGS: &str = "設定…";
const TEXT_CASUAL: &str = "口語（輕聲ㄉㄜ一律「的」）";
const TEXT_FORMAL: &str = "正式（的／地／得由模型判斷）";

#[implement(ITfLangBarItemButton, ITfLangBarItem, ITfSource)]
pub struct ModeButton {
    state: Shared,
}

impl ModeButton {
    pub(crate) fn new(state: Shared) -> Self {
        ModeButton { state }
    }

    fn select(&self, id: u32) {
        match id {
            MENU_SETTINGS => service::open_settings(),
            MENU_CASUAL => service::choose_style(&self.state, false),
            MENU_FORMAL => service::choose_style(&self.state, true),
            _ => {}
        }
    }

    /// Our own popup menu at the click, for when Windows only reports it.
    fn popup_menu(&self, at: POINT) {
        let formal = service::is_formal(&self.state);
        let checked = |on: bool| if on { MF_STRING | MF_CHECKED } else { MF_STRING };
        let picked = unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let text = |s: &str| crate::wide_z(s);
            let (settings, casual, formal_text) =
                (text(TEXT_SETTINGS), text(TEXT_CASUAL), text(TEXT_FORMAL));
            let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS as usize, PCWSTR(settings.as_ptr()));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, checked(!formal), MENU_CASUAL as usize, PCWSTR(casual.as_ptr()));
            let _ =
                AppendMenuW(menu, checked(formal), MENU_FORMAL as usize, PCWSTR(formal_text.as_ptr()));
            // A menu needs an owner window on this thread; a hidden one of
            // our own keeps the application's windows out of it.
            let owner = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
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
            .unwrap_or(HWND::default());
            // So that a click elsewhere closes the menu.
            let _ = SetForegroundWindow(owner);
            let picked = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                at.x,
                at.y,
                None,
                owner,
                None,
            );
            let _ = DestroyMenu(menu);
            let _ = DestroyWindow(owner);
            picked.0 as u32
        };
        self.select(picked);
    }
}

/// A small square icon with one character on it. The caller owns it.
fn draw_icon(chinese: bool) -> Result<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let (background, label) = if chinese { (0x00B0_6C2B, "中") } else { (0x0060_6060, "英") };
    unsafe {
        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size, // top-down
                biPlanes: 1,
                biBitCount: 32,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut bits = std::ptr::null_mut();
        let colour = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0);
        let Ok(colour) = colour else {
            let _ = DeleteDC(dc);
            return Err(E_FAIL.into());
        };
        let old_bitmap = SelectObject(dc, colour.into());
        let mut area = RECT { left: 0, top: 0, right: size, bottom: size };
        let brush = CreateSolidBrush(COLORREF(background));
        FillRect(dc, &area, brush);
        let _ = DeleteObject(brush.into());
        let font = CreateFontW(
            -(size * 13 / 16),
            0,
            0,
            0,
            600,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            Default::default(),
            Default::default(),
            CLEARTYPE_QUALITY,
            0,
            w!("Microsoft JhengHei UI"),
        );
        let old_font = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00FF_FFFF));
        let mut text = wide(label);
        DrawTextW(dc, &mut text, &mut area, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOCLIP);
        SelectObject(dc, old_font);
        let _ = DeleteObject(font.into());
        SelectObject(dc, old_bitmap);
        let _ = DeleteDC(dc);

        // GDI leaves the alpha channel at zero: make every pixel opaque.
        if !bits.is_null() {
            let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
            for pixel in pixels {
                *pixel |= 0xFF00_0000;
            }
        }
        let mask = CreateBitmap(size, size, 1, 1, None);
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: colour,
        });
        let _ = DeleteObject(mask.into());
        let _ = DeleteObject(colour.into());
        icon
    }
}

impl ITfLangBarItem_Impl for ModeButton_Impl {
    fn GetInfo(&self, info: *mut TF_LANGBARITEMINFO) -> Result<()> {
        let Some(info) = (unsafe { info.as_mut() }) else {
            return Err(E_INVALIDARG.into());
        };
        *info = TF_LANGBARITEMINFO {
            clsidService: CLSID_TEXT_SERVICE,
            // The item Windows shows in the taskbar's input indicator.
            guidItem: GUID_LBI_INPUTMODE,
            dwStyle: TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_SHOWNINTRAY,
            ulSort: 0,
            szDescription: [0; 32],
        };
        for (slot, unit) in info.szDescription.iter_mut().zip("Bopogram".encode_utf16()) {
            *slot = unit;
        }
        Ok(())
    }

    fn GetStatus(&self) -> Result<u32> {
        Ok(0)
    }

    /// The input indicator decides where the button appears; there is
    /// nothing to hide on our side, and refusing could keep it hidden.
    fn Show(&self, _show: BOOL) -> Result<()> {
        Ok(())
    }

    fn GetTooltipString(&self) -> Result<BSTR> {
        Ok(BSTR::from("Bopogram 注音：左鍵切換中／英，右鍵開啟選單"))
    }
}

impl ITfLangBarItemButton_Impl for ModeButton_Impl {
    fn OnClick(&self, click: TfLBIClick, at: &POINT, _area: *const RECT) -> Result<()> {
        guard(Ok(()), || {
            if click == TF_LBI_CLK_LEFT {
                service::toggle_mode(&self.state);
            } else if click == TF_LBI_CLK_RIGHT {
                self.popup_menu(*at);
            }
            Ok(())
        })
    }

    fn InitMenu(&self, menu: Ref<ITfMenu>) -> Result<()> {
        guard(Ok(()), || {
            let Some(menu) = menu.as_ref() else {
                return Err(E_INVALIDARG.into());
            };
            let formal = service::is_formal(&self.state);
            let checked = |on: bool| if on { TF_LBMENUF_CHECKED } else { 0 };
            let add = |id: u32, flags: u32, text: &str| unsafe {
                menu.AddMenuItem(
                    id,
                    flags,
                    Default::default(),
                    Default::default(),
                    &wide(text),
                    std::ptr::null_mut(),
                )
            };
            add(MENU_SETTINGS, 0, TEXT_SETTINGS)?;
            add(0, TF_LBMENUF_SEPARATOR, "")?;
            add(MENU_CASUAL, checked(!formal), TEXT_CASUAL)?;
            add(MENU_FORMAL, checked(formal), TEXT_FORMAL)
        })
    }

    fn OnMenuSelect(&self, id: u32) -> Result<()> {
        guard(Ok(()), || {
            self.select(id);
            Ok(())
        })
    }

    fn GetIcon(&self) -> Result<HICON> {
        guard(Err(E_FAIL.into()), || draw_icon(service::is_chinese(&self.state)))
    }

    fn GetText(&self) -> Result<BSTR> {
        Ok(BSTR::from(if service::is_chinese(&self.state) { "中" } else { "英" }))
    }
}

impl ITfSource_Impl for ModeButton_Impl {
    fn AdviseSink(&self, iid: *const GUID, sink: Ref<windows::core::IUnknown>) -> Result<u32> {
        if iid.is_null() || unsafe { *iid } != ITfLangBarItemSink::IID {
            return Err(CONNECT_E_CANNOTCONNECT.into());
        }
        let sink: ITfLangBarItemSink = sink.ok()?.cast()?;
        if service::has_button_sink(&self.state) {
            return Err(CONNECT_E_ADVISELIMIT.into());
        }
        service::set_button_sink(&self.state, Some(sink));
        Ok(SINK_COOKIE)
    }

    fn UnadviseSink(&self, cookie: u32) -> Result<()> {
        if cookie != SINK_COOKIE || !service::has_button_sink(&self.state) {
            return Err(CONNECT_E_NOCONNECTION.into());
        }
        service::set_button_sink(&self.state, None);
        Ok(())
    }
}
