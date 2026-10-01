use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

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
    pub wrapped: bool,
    pub text: String,
    pub cells: Vec<CaptureCell>,
}

#[derive(Debug, Serialize)]
pub struct CaptureCell {
    pub col: u16,
    pub width: u16,
    pub continuation: bool,
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

pub fn unix_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
