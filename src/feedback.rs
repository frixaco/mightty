use gpui::Window;
use serde::Serialize;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct CapturePaths {
    pub directory: PathBuf,
    pub json_path: PathBuf,
    pub png_path: Option<PathBuf>,
    pub pixel_capture_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TerminalCapture {
    pub captured_unix_ms: u128,
    pub terminal_size: GridSize,
    pub cell_size_px: SizePx,
    pub font: FontCapture,
    pub colors: CaptureColors,
    pub cursor: Option<CaptureCursor>,
    pub rows: Vec<CaptureRow>,
}

#[derive(Debug, Serialize)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Serialize)]
pub struct SizePx {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Serialize)]
pub struct FontCapture {
    pub family: String,
    pub size_px: f32,
}

#[derive(Debug, Serialize)]
pub struct CaptureColors {
    pub foreground: RgbHex,
    pub background: RgbHex,
    pub cursor: Option<RgbHex>,
}

#[derive(Debug, Serialize)]
pub struct CaptureCursor {
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Serialize)]
pub struct CaptureRow {
    pub index: u16,
    pub text: String,
    pub cells: Vec<CaptureCell>,
}

#[derive(Debug, Serialize)]
pub struct CaptureCell {
    pub col: u16,
    pub text: String,
    pub fg: RgbHex,
    pub bg: Option<RgbHex>,
    pub bold: bool,
    pub italic: bool,
    pub underline: String,
    pub inverse: bool,
    pub strikethrough: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct RgbHex {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub hex: u32,
}

impl RgbHex {
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self {
            r,
            g,
            b,
            hex: ((r as u32) << 16) | ((g as u32) << 8) | (b as u32),
        }
    }
}

pub fn write_capture(capture: &TerminalCapture, window: &Window) -> io::Result<CapturePaths> {
    let captures_dir = PathBuf::from("captures");
    fs::create_dir_all(&captures_dir)?;

    let stem = format!("terminal-capture-{}", capture.captured_unix_ms);
    let directory = captures_dir.join(&stem);
    fs::create_dir_all(&directory)?;

    let json_path = directory.join("capture.json");
    let json_file = File::create(&json_path)?;
    serde_json::to_writer_pretty(json_file, capture).map_err(io::Error::other)?;

    #[cfg(windows)]
    {
        let png_path = directory.join("capture.png");
        match capture_window_png(window, &png_path) {
            Ok(()) => Ok(CapturePaths {
                directory,
                json_path,
                png_path: Some(png_path),
                pixel_capture_error: None,
            }),
            Err(err) => Ok(CapturePaths {
                directory,
                json_path,
                png_path: None,
                pixel_capture_error: Some(err.to_string()),
            }),
        }
    }

    #[cfg(not(windows))]
    {
        let _ = window;
        Ok(CapturePaths {
            directory,
            json_path,
            png_path: None,
            pixel_capture_error: Some("pixel capture is only implemented on Windows".to_string()),
        })
    }
}

pub fn unix_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(windows)]
fn capture_window_png(window: &Window, path: &Path) -> io::Result<()> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let raw_handle = HasWindowHandle::window_handle(window)
        .map_err(|err| io::Error::other(format!("window handle unavailable: {err:?}")))?;
    let RawWindowHandle::Win32(handle) = raw_handle.as_raw() else {
        return Err(io::Error::other("window is not backed by a Win32 handle"));
    };

    capture_hwnd_png(handle.hwnd.get(), path)
}

#[cfg(windows)]
fn capture_hwnd_png(hwnd: isize, path: &Path) -> io::Result<()> {
    use std::mem::size_of;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap,
        CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, GetDC, HBITMAP,
        HGDIOBJ, RGBQUAD, ReleaseDC, SRCCOPY, SelectObject,
    };
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

    unsafe {
        let hwnd = hwnd as HWND;

        let mut client_rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetClientRect(hwnd, &mut client_rect) == 0 {
            return Err(io::Error::last_os_error());
        }

        let width = client_rect.right - client_rect.left;
        let height = client_rect.bottom - client_rect.top;
        if width <= 0 || height <= 0 {
            return Err(io::Error::other("window client area is empty"));
        }

        let mut origin = POINT { x: 0, y: 0 };
        if ClientToScreen(hwnd, &mut origin) == 0 {
            return Err(io::Error::last_os_error());
        }

        let screen_dc = GetDC(null_mut());
        if screen_dc.is_null() {
            return Err(io::Error::last_os_error());
        }

        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.is_null() {
            ReleaseDC(null_mut(), screen_dc);
            return Err(io::Error::last_os_error());
        }

        let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
        if bitmap.is_null() {
            DeleteDC(mem_dc);
            ReleaseDC(null_mut(), screen_dc);
            return Err(io::Error::last_os_error());
        }

        let old_bitmap = SelectObject(mem_dc, bitmap as HGDIOBJ);
        if old_bitmap.is_null() {
            DeleteObject(bitmap as HGDIOBJ);
            DeleteDC(mem_dc);
            ReleaseDC(null_mut(), screen_dc);
            return Err(io::Error::last_os_error());
        }

        if BitBlt(mem_dc, 0, 0, width, height, screen_dc, origin.x, origin.y, SRCCOPY) == 0 {
            SelectObject(mem_dc, old_bitmap);
            DeleteObject(bitmap as HGDIOBJ);
            DeleteDC(mem_dc);
            ReleaseDC(null_mut(), screen_dc);
            return Err(io::Error::last_os_error());
        }

        let mut bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: (width * height * 4) as u32,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbReserved: 0,
            }],
        };

        let mut bgra = vec![0u8; (width as usize) * (height as usize) * 4];
        let copied = GetDIBits(
            mem_dc,
            bitmap as HBITMAP,
            0,
            height as u32,
            bgra.as_mut_ptr().cast(),
            &mut bitmap_info,
            DIB_RGB_COLORS,
        );

        SelectObject(mem_dc, old_bitmap);
        DeleteObject(bitmap as HGDIOBJ);
        DeleteDC(mem_dc);
        ReleaseDC(null_mut(), screen_dc);

        if copied == 0 {
            return Err(io::Error::last_os_error());
        }

        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        image::save_buffer(
            path,
            &bgra,
            width as u32,
            height as u32,
            image::ColorType::Rgba8,
        )
        .map_err(io::Error::other)
    }
}
