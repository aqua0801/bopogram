//! The icons, drawn here with GDI (no image files from elsewhere):
//!
//! - the mode icon in the taskbar: 中 or 英 in the taskbar's own text
//!   colour (light or dark theme), with a small bar under it in full-width
//!   mode. Drawn when the taskbar asks, so it always matches the theme.
//! - the input method's icon in the list of keyboards: ㄅ on a rounded blue
//!   square. Written once to `res/bopogram.ico` by the ignored test below
//!   (`cargo test -p bopogram-ime --lib icon -- --ignored`); `regsvr32`
//!   points Windows at the copy installed beside the DLL.

use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{COLORREF, E_FAIL, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DeleteDC,
    DeleteObject, DrawTextW, FillRect, SelectObject, SetBkMode, SetTextColor, ANTIALIASED_QUALITY,
    BITMAPINFO, BITMAPINFOHEADER, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CENTER, DT_NOCLIP,
    DT_SINGLELINE, DT_VCENTER, FONT_QUALITY, HDC, TRANSPARENT,
};
// Only for drawing the icon file.
#[cfg(test)]
use windows::Win32::{
    Foundation::POINT,
    Graphics::Gdi::{GetStockObject, Polygon, RoundRect, NULL_PEN},
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON,
};

use crate::wide;

/// Blue of the input method's icon (0xRRGGBB).
#[cfg(test)] // only the icon file needs it
const BRAND_BLUE: u32 = 0x2B6CB0;
const FONT: PCWSTR = w!("Microsoft JhengHei UI");

/// How much of each pixel of a `size` square `draw` covers, from 0 to 1.
/// `draw` paints white on black at `scale` times the size; the result is
/// averaged back down, which smooths shapes GDI draws without antialiasing.
fn coverage(size: i32, scale: i32, draw: impl FnOnce(HDC, i32)) -> Vec<f32> {
    let big = size * scale;
    let mut result = vec![0.0; (size * size) as usize];
    unsafe {
        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: big,
                biHeight: -big, // top-down
                biPlanes: 1,
                biBitCount: 32,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            return result;
        };
        let old = SelectObject(dc, bitmap.into());
        // A new DIB section is all zeros: black already.
        draw(dc, big);
        if !bits.is_null() {
            let pixels = std::slice::from_raw_parts(bits as *const u32, (big * big) as usize);
            let samples = (scale * scale) as f32;
            for y in 0..big {
                for x in 0..big {
                    let level = (pixels[(y * big + x) as usize] >> 8 & 0xFF) as f32 / 255.0;
                    result[((y / scale) * size + x / scale) as usize] += level / samples;
                }
            }
        }
        SelectObject(dc, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
    }
    result
}

/// Write `text` in white, centred in `area`.
fn text(dc: HDC, label: &str, px: i32, weight: i32, quality: FONT_QUALITY, mut area: RECT) {
    unsafe {
        let font = CreateFontW(
            -px,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            Default::default(),
            Default::default(),
            quality,
            0,
            FONT,
        );
        let old = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00FF_FFFF));
        let mut label = wide(label);
        DrawTextW(dc, &mut label, &mut area, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOCLIP);
        SelectObject(dc, old);
        let _ = DeleteObject(font.into());
    }
}

fn fill_white(dc: HDC, area: RECT) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(0x00FF_FFFF));
        FillRect(dc, &area, brush);
        let _ = DeleteObject(brush.into());
    }
}

/// One pixel, 0xAARRGGBB, not premultiplied.
fn pixel(rgb: u32, alpha: f32) -> u32 {
    let alpha = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
    alpha << 24 | (rgb & 0xFF_FFFF)
}

/// The mode icon's pixels, top row first.
pub(crate) fn mode_pixels(size: i32, chinese: bool, full_width: bool, dark_taskbar: bool) -> Vec<u32> {
    let colour = if dark_taskbar { 0xFFFFFF } else { 0x1A1A1A };
    let label = if chinese { "中" } else { "英" };
    // Hinted greyscale text at the real size stays sharpest at 16 pixels.
    let shape = coverage(size, 1, |dc, side| {
        let bar = if full_width { (side / 16).max(1) } else { 0 };
        let glyph = RECT { left: 0, top: 0, right: side, bottom: side - 2 * bar };
        text(dc, label, (side - 2 * bar) * 7 / 8, 400, ANTIALIASED_QUALITY, glyph);
        if full_width {
            let inset = side / 4;
            fill_white(dc, RECT { left: inset, top: side - bar, right: side - inset, bottom: side });
        }
    });
    shape.iter().map(|&covered| pixel(colour, covered)).collect()
}

/// The input method's own icon: ㄅ on a rounded blue square.
#[cfg(test)] // only the icon file needs it
fn brand_pixels(size: i32) -> Vec<u32> {
    const SCALE: i32 = 4;
    let square = coverage(size, SCALE, |dc, side| unsafe {
        let brush = CreateSolidBrush(COLORREF(0x00FF_FFFF));
        let old_brush = SelectObject(dc, brush.into());
        let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        let margin = side / 16;
        let round = side / 4;
        let _ = RoundRect(dc, margin, margin, side - margin + 1, side - margin + 1, round, round);
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        let _ = DeleteObject(brush.into());
    });
    // ㄅ in strokes on whole pixels, so that it stays sharp at 16 pixels.
    let at = |fraction: f32| (fraction * size as f32).round() as i32;
    let stroke = (size as f32 * 0.11).round().max(2.0) as i32;
    let (left, right, top, bottom) = (at(0.29), at(0.69), at(0.42), at(0.80));
    let (slant_x, slant_y) = (at(0.37), at(0.20));
    let glyph = coverage(size, SCALE, |dc, _| unsafe {
        let s = |v: i32| v * SCALE;
        let rect = |l: i32, t: i32, r: i32, b: i32| RECT { left: s(l), top: s(t), right: s(r), bottom: s(b) };
        fill_white(dc, rect(left, top, right, top + stroke));
        fill_white(dc, rect(right - stroke, top, right, bottom));
        fill_white(dc, rect(at(0.56), bottom - stroke, right, bottom));
        let brush = CreateSolidBrush(COLORREF(0x00FF_FFFF));
        let old_brush = SelectObject(dc, brush.into());
        let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        let slant = [
            POINT { x: s(slant_x), y: s(slant_y) },
            POINT { x: s(slant_x + stroke), y: s(slant_y) },
            POINT { x: s(left + stroke), y: s(top + 1) },
            POINT { x: s(left), y: s(top + 1) },
        ];
        let _ = Polygon(dc, &slant);
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        let _ = DeleteObject(brush.into());
    });
    square
        .iter()
        .zip(&glyph)
        .map(|(&square, &glyph)| {
            // White over blue, both inside the square.
            let mix = |channel: u32| {
                let blue = (BRAND_BLUE >> channel & 0xFF) as f32;
                (blue + (255.0 - blue) * glyph).round() as u32
            };
            pixel(mix(16) << 16 | mix(8) << 8 | mix(0), square)
        })
        .collect()
}

/// An icon from pixels (0xAARRGGBB, top row first). The caller owns it.
fn icon_from(size: i32, pixels: &[u32]) -> Result<HICON> {
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut bits = std::ptr::null_mut();
        let colour = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .map_err(|_| windows::core::Error::from(E_FAIL))?;
        if !bits.is_null() {
            std::slice::from_raw_parts_mut(bits as *mut u32, pixels.len()).copy_from_slice(pixels);
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

/// The mode icon for the taskbar, at the size it uses. The caller owns it.
pub(crate) fn mode_icon(chinese: bool, full_width: bool) -> Result<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let dark = crate::popup::is_dark(w!("SystemUsesLightTheme"));
    icon_from(size, &mode_pixels(size, chinese, full_width, dark))
}

/// An `.ico` file holding the given sizes, as 32-bit images with alpha.
#[cfg(test)] // only the icon file needs it
fn ico_file(images: &[(i32, Vec<u32>)]) -> Vec<u8> {
    let mut header = Vec::new();
    let mut data = Vec::new();
    header.extend_from_slice(&[0, 0, 1, 0]);
    header.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (size, pixels) in images {
        let side = *size as usize;
        let mask_row = side.div_ceil(32) * 4;
        let mut image = Vec::new();
        // BITMAPINFOHEADER; the height counts the colour and mask halves.
        image.extend_from_slice(&40u32.to_le_bytes());
        image.extend_from_slice(&(*size).to_le_bytes());
        image.extend_from_slice(&(2 * *size).to_le_bytes());
        image.extend_from_slice(&1u16.to_le_bytes());
        image.extend_from_slice(&32u16.to_le_bytes());
        image.extend_from_slice(&[0; 24]);
        // Bottom row first.
        for row in (0..side).rev() {
            for &argb in &pixels[row * side..(row + 1) * side] {
                image.extend_from_slice(&argb.to_le_bytes()); // B G R A
            }
        }
        for row in (0..side).rev() {
            let mut bits = vec![0u8; mask_row];
            for x in 0..side {
                if pixels[row * side + x] >> 24 == 0 {
                    bits[x / 8] |= 0x80 >> (x % 8);
                }
            }
            image.extend_from_slice(&bits);
        }
        let dimension = if side >= 256 { 0 } else { side as u8 };
        header.extend_from_slice(&[dimension, dimension, 0, 0]);
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&32u16.to_le_bytes());
        header.extend_from_slice(&(image.len() as u32).to_le_bytes());
        header.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += image.len();
        data.extend_from_slice(&image);
    }
    header.extend_from_slice(&data);
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_icon_is_drawn_in_the_taskbar_colour() {
        let dark = mode_pixels(16, true, false, true);
        let light = mode_pixels(16, true, false, false);
        let opaque = |pixels: &[u32]| pixels.iter().filter(|&&p| p >> 24 > 0x80).count();
        assert!(opaque(&dark) > 20, "the character is there");
        assert!(dark.iter().filter(|&&p| p >> 24 > 0).all(|&p| p & 0xFF_FFFF == 0xFF_FFFF));
        assert!(light.iter().filter(|&&p| p >> 24 > 0).all(|&p| p & 0xFF_FFFF == 0x1A_1A1A));
        assert_eq!(dark[0] >> 24, 0, "transparent around it");
        let full = mode_pixels(16, true, true, true);
        assert!(full[15 * 16 + 8] >> 24 == 0xFF, "the full-width bar");
    }

    #[test]
    fn ico_layout() {
        let file = ico_file(&[(16, vec![0xFF00_0000; 256]), (32, vec![0; 1024])]);
        assert_eq!(&file[..6], &[0, 0, 1, 0, 2, 0]);
        let first = u32::from_le_bytes(file[18..22].try_into().unwrap()) as usize;
        assert_eq!(first, 6 + 32);
        assert_eq!(first, 38);
        // 40 header + 16*16*4 colour + 16 rows of 4 mask bytes.
        assert_eq!(u32::from_le_bytes(file[14..18].try_into().unwrap()), 40 + 1024 + 64);
    }

    /// Writes `res/bopogram.ico`, and previews to look at in `%TEMP%`.
    #[test]
    #[ignore = "writes the icon file"]
    fn write_icon_file() {
        let sizes = [16, 20, 24, 32, 40, 48, 64];
        let images: Vec<_> = sizes.iter().map(|&size| (size, brand_pixels(size))).collect();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("res").join("bopogram.ico");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, ico_file(&images)).unwrap();

        let previews = [
            ("mode-dark", mode_pixels(24, true, false, true), 0xFF20_2020u32),
            ("mode-light", mode_pixels(24, false, true, false), 0xFFF3_F3F3),
            ("brand", brand_pixels(48), 0xFFFF_FFFF),
        ];
        for (name, pixels, background) in previews {
            let size = (pixels.len() as f64).sqrt() as i32;
            let preview = ico_file(&[(size, blend(&pixels, background))]);
            std::fs::write(std::env::temp_dir().join(format!("bopogram-icon-{name}.ico")), preview).unwrap();
        }
    }

    /// Pixels laid over a solid background, for previews.
    fn blend(pixels: &[u32], background: u32) -> Vec<u32> {
        pixels
            .iter()
            .map(|&p| {
                let a = (p >> 24) as f32 / 255.0;
                let channel = |shift: u32| {
                    let fg = (p >> shift & 0xFF) as f32;
                    let bg = (background >> shift & 0xFF) as f32;
                    (fg * a + bg * (1.0 - a)).round() as u32
                };
                0xFF00_0000 | channel(16) << 16 | channel(8) << 8 | channel(0)
            })
            .collect()
    }
}
