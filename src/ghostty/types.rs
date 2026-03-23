//! Types for libghostty-vt FFI bindings

use std::ffi::c_void;

/// Allocator structure for custom memory management
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyAllocator {
    pub userdata: *mut c_void,
    pub alloc: Option<unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void>,
    pub resize: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, usize, usize) -> *mut c_void>,
    pub free: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, usize)>,
}

/// RGB color
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Cell style (SGR attributes)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyStyle {
    pub size: usize,
    pub bold: bool,
    pub italic: bool,
    pub underline: u8,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub fg_color: GhosttyColorRgb,
    pub bg_color: GhosttyColorRgb,
    pub fg_color_type: u8,
    pub bg_color_type: u8,
}

/// Terminal modes
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyMode {
    /// Application cursor keys mode (DECCKM)
    ApplicationCursor = 1,
    /// Insert mode (IRM)
    Insert = 4,
    /// Send/receive mode (SRM)
    SendReceive = 7,
    /// Linefeed mode (LNM)
    Linefeed = 20,
    /// Cursor visible (DECTCEM)
    CursorVisible = 25,
    /// Alternate screen buffer
    AlternateScreen = 47,
    /// Bracketed paste mode
    BracketedPaste = 2004,
    /// Focus event reporting
    FocusEvent = 1004,
    /// Mouse tracking - X10
    MouseX10 = 9,
    /// Mouse tracking - normal
    MouseNormal = 1000,
    /// Mouse tracking - button
    MouseButton = 1002,
    /// Mouse tracking - any event
    MouseAnyEvent = 1003,
    /// Mouse format - UTF-8
    MouseUtf8 = 1005,
    /// Mouse format - SGR
    MouseSgr = 1006,
    /// Mouse format - URXVT
    MouseUrxvt = 1015,
    /// Mouse format - SGR pixels
    MouseSgrPixels = 1016,
}

// ============================================================================
// Screen Buffer Types
// ============================================================================

/// Cell attributes (SGR styles)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attributes {
    pub bold: bool,
    pub italic: bool,
    pub underline: u8,
    pub strikethrough: bool,
    pub inverse: bool,
    pub blink: bool,
}

/// Cell color
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// Default terminal color
    Default,
    /// Palette color (0-255)
    Palette(u8),
    /// True color RGB
    Rgb(u8, u8, u8),
}

impl Default for Color {
    fn default() -> Self {
        Color::Default
    }
}

/// Terminal cell with character and styling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell {
    pub char: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attributes,
}

/// Screen buffer containing terminal cell data
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenBuffer {
    pub rows: u16,
    pub cols: u16,
    pub cells: Vec<Vec<Cell>>,
    pub cursor: (u16, u16),
}
