//! Rust FFI bindings for libghostty-vt
//!
//! libghostty-vt is a virtual terminal emulator library that provides
//! functionality for parsing terminal escape sequences and maintaining
//! terminal state.

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::ffi::{c_int, c_void};
use std::fmt::Debug;
use std::ptr::NonNull;

// Re-export types
mod types;
pub use types::*;

/// Opaque handle to a terminal instance
#[derive(Debug)]
pub struct Terminal {
    ptr: NonNull<GhosttyTerminalInner>,
    cached_buffer: Option<ScreenBuffer>,
}

/// Opaque inner type for the terminal handle
#[repr(C)]
pub struct GhosttyTerminalInner {
    _private: [u8; 0],
}

/// Result codes for libghostty-vt operations
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhosttyResult {
    /// Operation completed successfully
    Success = 0,
    /// Operation failed due to failed allocation
    OutOfMemory = -1,
    /// Operation failed due to invalid value
    InvalidValue = -2,
    /// Operation failed because the provided buffer was too small
    OutOfSpace = -3,
}

impl GhosttyResult {
    pub fn is_success(&self) -> bool {
        matches!(self, GhosttyResult::Success)
    }
}

/// Terminal initialization options
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyTerminalOptions {
    /// Terminal width in cells. Must be greater than zero.
    pub cols: u16,
    /// Terminal height in cells. Must be greater than zero.
    pub rows: u16,
    /// Maximum number of lines to keep in scrollback history.
    pub max_scrollback: usize,
}

impl Default for GhosttyTerminalOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            max_scrollback: 1000,
        }
    }
}

/// Scroll viewport behavior tag
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyTerminalScrollViewportTag {
    /// Scroll to the top of the scrollback
    Top,
    /// Scroll to the bottom (active area)
    Bottom,
    /// Scroll by a delta amount (up is negative)
    Delta,
}

/// Scroll viewport value (union)
#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyTerminalScrollViewportValue {
    /// Scroll delta (only used with Delta). Up is negative.
    pub delta: isize,
    /// Padding for ABI compatibility
    _padding: [u64; 2],
}

/// Tagged union for scroll viewport behavior
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalScrollViewport {
    pub tag: GhosttyTerminalScrollViewportTag,
    pub value: GhosttyTerminalScrollViewportValue,
}

/// Terminal screen identifier
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyTerminalScreen {
    /// The primary (normal) screen
    Primary = 0,
    /// The alternate screen
    Alternate = 1,
}

/// Scrollbar state for the terminal viewport
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyTerminalScrollbar {
    /// Total size of the scrollable area in rows
    pub total: u64,
    /// Offset into the total area that the viewport is at
    pub offset: u64,
    /// Length of the visible area in rows
    pub len: u64,
}

/// Cell content tag
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyCellContentTag {
    /// A single codepoint (may be zero for empty)
    Codepoint = 0,
    /// A codepoint that is part of a multi-codepoint grapheme cluster
    CodepointGrapheme = 1,
    /// No text; background color from palette
    BgColorPalette = 2,
    /// No text; background color as RGB
    BgColorRgb = 3,
}

/// Cell wide property
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyCellWide {
    /// Not a wide character, cell width 1
    Narrow = 0,
    /// Wide character, cell width 2
    Wide = 1,
    /// Spacer after wide character. Do not render
    SpacerTail = 2,
    /// Spacer at end of soft-wrapped line for a wide character
    SpacerHead = 3,
}

/// Cell data types for ghostty_cell_get
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyCellData {
    Invalid = 0,
    Codepoint = 1,
    ContentTag = 2,
    Wide = 3,
    HasText = 4,
    HasStyling = 5,
    StyleId = 6,
    HasHyperlink = 7,
    Protected = 8,
    SemanticContent = 9,
    ColorPalette = 10,
    ColorRgb = 11,
}

/// Row data types for ghostty_row_get
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyRowData {
    Invalid = 0,
    Wrap = 1,
    WrapContinuation = 2,
    Grapheme = 3,
    Styled = 4,
    Hyperlink = 5,
    SemanticPrompt = 6,
    KittyVirtualPlaceholder = 7,
    Dirty = 8,
}

/// Point in the terminal grid with coordinate type tag
#[repr(C)]
#[derive(Clone)]
pub struct GhosttyPoint {
    pub tag: GhosttyPointTag,
    pub value: GhosttyPointValue,
}

/// Point coordinate values
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyPointCoordinate {
    pub x: u16,
    pub y: u32,
}

/// Point value union
#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyPointValue {
    pub coordinate: GhosttyPointCoordinate,
    _padding: [u64; 2],
}

impl Debug for GhosttyPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GhosttyPoint")
            .field("tag", &self.tag)
            .finish()
    }
}

impl Debug for GhosttyPointValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GhosttyPointValue").finish()
    }
}

/// Point coordinate type tag
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyPointTag {
    /// Point is in active area coordinates (viewport)
    Active = 0,
    /// Point is in viewport coordinates (may be outside active area)
    Viewport = 1,
    /// Point is in screen coordinates (primary or alternate screen)
    Screen = 2,
    /// Point is in history/scrollback coordinates
    History = 3,
}

/// Grid reference - resolved position in terminal grid
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyGridRef {
    pub size: usize,
    pub node: *mut c_void,
    pub x: u16,
    pub y: u16,
}

/// Opaque cell type (just a handle)
pub type GhosttyCell = u64;

/// Opaque row type (just a handle)
pub type GhosttyRow = u64;

#[cfg_attr(windows, link(name = "ghostty-vt", kind = "raw-dylib"))]
#[cfg_attr(not(windows), link(name = "ghostty-vt", kind = "dylib"))]
unsafe extern "C" {
    /// Create a new terminal instance
    pub fn ghostty_terminal_new(
        allocator: *const c_void,
        terminal: *mut *mut GhosttyTerminalInner,
        options: GhosttyTerminalOptions,
    ) -> GhosttyResult;

    /// Free a terminal instance
    pub fn ghostty_terminal_free(terminal: *mut GhosttyTerminalInner);

    /// Perform a full reset of the terminal (RIS)
    pub fn ghostty_terminal_reset(terminal: *mut GhosttyTerminalInner);

    /// Resize the terminal to the given dimensions
    pub fn ghostty_terminal_resize(
        terminal: *mut GhosttyTerminalInner,
        cols: u16,
        rows: u16,
    ) -> GhosttyResult;

    /// Write VT-encoded data to the terminal for processing
    pub fn ghostty_terminal_vt_write(
        terminal: *mut GhosttyTerminalInner,
        data: *const u8,
        len: usize,
    );

    /// Scroll the terminal viewport
    pub fn ghostty_terminal_scroll_viewport(
        terminal: *mut GhosttyTerminalInner,
        behavior: GhosttyTerminalScrollViewport,
    );

    /// Get data from a terminal instance
    pub fn ghostty_terminal_get(
        terminal: *mut GhosttyTerminalInner,
        data: c_int,
        out: *mut c_void,
    ) -> GhosttyResult;

    /// Resolve a point to a grid reference
    pub fn ghostty_terminal_grid_ref(
        terminal: *mut GhosttyTerminalInner,
        point: GhosttyPoint,
        out_ref: *mut GhosttyGridRef,
    ) -> GhosttyResult;

    /// Get cell from grid reference
    pub fn ghostty_grid_ref_cell(
        grid_ref: *const GhosttyGridRef,
        out_cell: *mut GhosttyCell,
    ) -> GhosttyResult;

    /// Get row from grid reference
    pub fn ghostty_grid_ref_row(
        grid_ref: *const GhosttyGridRef,
        out_row: *mut GhosttyRow,
    ) -> GhosttyResult;

    /// Get style from grid reference
    pub fn ghostty_grid_ref_style(
        grid_ref: *const GhosttyGridRef,
        out_style: *mut GhosttyStyle,
    ) -> GhosttyResult;

    /// Get data from a cell
    pub fn ghostty_cell_get(
        cell: GhosttyCell,
        data: GhosttyCellData,
        out: *mut c_void,
    ) -> GhosttyResult;

    /// Get data from a row
    pub fn ghostty_row_get(
        row: GhosttyRow,
        data: GhosttyRowData,
        out: *mut c_void,
    ) -> GhosttyResult;
}

impl Terminal {
    /// Create a new terminal with the given options
    pub fn new(options: GhosttyTerminalOptions) -> Result<Self, GhosttyResult> {
        let mut ptr: *mut GhosttyTerminalInner = std::ptr::null_mut();
        let result = unsafe { ghostty_terminal_new(std::ptr::null(), &mut ptr, options) };

        if result == GhosttyResult::Success {
            Ok(Terminal {
                ptr: NonNull::new(ptr)
                    .expect("Terminal pointer was null after successful creation"),
                cached_buffer: None,
            })
        } else {
            Err(result)
        }
    }

    /// Create a new terminal with default options (80x24, 1000 scrollback)
    pub fn new_default() -> Result<Self, GhosttyResult> {
        Self::new(GhosttyTerminalOptions::default())
    }

    /// Write data to the terminal's VT parser
    pub fn write(&mut self, data: &[u8]) {
        unsafe {
            ghostty_terminal_vt_write(self.ptr.as_ptr(), data.as_ptr(), data.len());
        }
        self.cached_buffer = None;
    }

    /// Write a string to the terminal
    pub fn write_str(&mut self, s: &str) {
        self.write(s.as_bytes());
    }

    /// Resize the terminal
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), GhosttyResult> {
        let result = unsafe { ghostty_terminal_resize(self.ptr.as_ptr(), cols, rows) };
        if result == GhosttyResult::Success {
            self.cached_buffer = None;
            Ok(())
        } else {
            Err(result)
        }
    }

    /// Reset the terminal to initial state
    pub fn reset(&mut self) {
        unsafe {
            ghostty_terminal_reset(self.ptr.as_ptr());
        }
        self.cached_buffer = None;
    }

    /// Get terminal dimensions
    pub fn size(&self) -> (u16, u16) {
        let mut cols: u16 = 0;
        let mut rows: u16 = 0;
        unsafe {
            ghostty_terminal_get(self.ptr.as_ptr(), 1, &mut cols as *mut _ as *mut c_void);
            ghostty_terminal_get(self.ptr.as_ptr(), 2, &mut rows as *mut _ as *mut c_void);
        }
        (cols, rows)
    }

    /// Get cursor position
    pub fn cursor_pos(&self) -> (u16, u16) {
        let mut x: u16 = 0;
        let mut y: u16 = 0;
        unsafe {
            ghostty_terminal_get(self.ptr.as_ptr(), 3, &mut x as *mut _ as *mut c_void);
            ghostty_terminal_get(self.ptr.as_ptr(), 4, &mut y as *mut _ as *mut c_void);
        }
        (x, y)
    }

    /// Check if cursor is visible
    pub fn cursor_visible(&self) -> bool {
        let mut visible: bool = false;
        unsafe {
            ghostty_terminal_get(self.ptr.as_ptr(), 7, &mut visible as *mut _ as *mut c_void);
        }
        visible
    }

    /// Get scrollbar state
    pub fn scrollbar(&self) -> Option<GhosttyTerminalScrollbar> {
        let mut scrollbar: GhosttyTerminalScrollbar = GhosttyTerminalScrollbar {
            total: 0,
            offset: 0,
            len: 0,
        };
        let result = unsafe {
            ghostty_terminal_get(
                self.ptr.as_ptr(),
                9,
                &mut scrollbar as *mut _ as *mut c_void,
            )
        };
        if result == GhosttyResult::Success {
            Some(scrollbar)
        } else {
            None
        }
    }

    // ============================================================================
    // Screen Buffer Methods
    // ============================================================================

    /// Read the entire screen buffer from libghostty
    pub fn read_screen(&mut self) -> &ScreenBuffer {
        let (cols, rows) = self.size();
        let cursor = self.cursor_pos();

        let mut cells: Vec<Vec<Cell>> = Vec::with_capacity(rows as usize);

        for row in 0..rows {
            let mut row_cells: Vec<Cell> = Vec::with_capacity(cols as usize);

            for col in 0..cols {
                if let Some(cell) = self.cell_at(row, col) {
                    row_cells.push(cell);
                } else {
                    row_cells.push(Cell::default());
                }
            }

            cells.push(row_cells);
        }

        self.cached_buffer = Some(ScreenBuffer {
            rows,
            cols,
            cells,
            cursor,
        });

        self.cached_buffer.as_ref().unwrap()
    }

    /// Get a single cell at the specified position (row, col)
    pub fn cell_at(&self, row: u16, col: u16) -> Option<Cell> {
        let (term_cols, term_rows) = self.size();
        if row >= term_rows || col >= term_cols {
            return None;
        }

        // x = column, y = row (as per ghostty API)
        let point = GhosttyPoint {
            tag: GhosttyPointTag::Active,
            value: GhosttyPointValue {
                coordinate: GhosttyPointCoordinate {
                    x: col,
                    y: row as u32,
                },
            },
        };

        let mut grid_ref = GhosttyGridRef {
            size: std::mem::size_of::<GhosttyGridRef>(),
            node: std::ptr::null_mut(),
            x: 0,
            y: 0,
        };

        let result = unsafe { ghostty_terminal_grid_ref(self.ptr.as_ptr(), point, &mut grid_ref) };

        if result != GhosttyResult::Success {
            return None;
        }

        // Get the cell handle
        let mut cell_handle: GhosttyCell = 0;
        let cell_result = unsafe { ghostty_grid_ref_cell(&grid_ref, &mut cell_handle) };

        if cell_result != GhosttyResult::Success {
            return None;
        }

        // Get the style
        let mut style: GhosttyStyle = unsafe { std::mem::zeroed() };
        let style_result = unsafe { ghostty_grid_ref_style(&grid_ref, &mut style) };
        if style_result != GhosttyResult::Success {
            return None;
        }

        // Extract cell data
        let mut codepoint: u32 = 0;
        unsafe {
            ghostty_cell_get(
                cell_handle,
                GhosttyCellData::Codepoint,
                &mut codepoint as *mut _ as *mut c_void,
            );
        }

        let char = std::char::from_u32(codepoint).unwrap_or('?');

        Some(Cell {
            char,
            fg: Self::ghostty_color_to_color(style.fg_color_type, style.fg_color),
            bg: Self::ghostty_color_to_color(style.bg_color_type, style.bg_color),
            attrs: Attributes {
                bold: style.bold,
                italic: style.italic,
                underline: style.underline,
                strikethrough: style.strikethrough,
                inverse: style.inverse,
                blink: style.blink,
            },
        })
    }

    /// Get cursor position in screen coordinates
    pub fn cursor_screen_pos(&self) -> (u16, u16) {
        self.cursor_pos()
    }

    /// Check if a cell has been modified since last screen read
    /// Note: This requires iterating all cells to check the dirty flag
    /// Note: Ghostty API uses x=row, y=col (swapped from standard terminal coordinates)
    pub fn cell_modified(&self, row: u16, col: u16) -> bool {
        let (term_cols, term_rows) = self.size();
        if row >= term_rows || col >= term_cols {
            return false;
        }

        // x = column, y = row (as per ghostty API)
        let point = GhosttyPoint {
            tag: GhosttyPointTag::Active,
            value: GhosttyPointValue {
                coordinate: GhosttyPointCoordinate {
                    x: col,
                    y: row as u32,
                },
            },
        };

        let mut grid_ref = GhosttyGridRef {
            size: std::mem::size_of::<GhosttyGridRef>(),
            node: std::ptr::null_mut(),
            x: 0,
            y: 0,
        };

        let result = unsafe { ghostty_terminal_grid_ref(self.ptr.as_ptr(), point, &mut grid_ref) };

        if result != GhosttyResult::Success {
            return false;
        }

        // Get the row and check if it's dirty
        let mut row_handle: GhosttyRow = 0;
        let row_result = unsafe { ghostty_grid_ref_row(&grid_ref, &mut row_handle) };

        if row_result != GhosttyResult::Success {
            return false;
        }

        let mut is_dirty: bool = false;
        unsafe {
            ghostty_row_get(
                row_handle,
                GhosttyRowData::Dirty,
                &mut is_dirty as *mut _ as *mut c_void,
            );
        }

        is_dirty
    }

    /// Convert Ghostty color to our Color type
    fn ghostty_color_to_color(color_type: u8, rgb: GhosttyColorRgb) -> Color {
        match color_type {
            0 => Color::Default,
            1 => Color::Palette(rgb.r),
            2 => Color::Rgb(rgb.r, rgb.g, rgb.b),
            _ => Color::Default,
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe {
            ghostty_terminal_free(self.ptr.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_creation() {
        let terminal = Terminal::new_default();
        assert!(terminal.is_ok());
    }

    #[test]
    fn test_terminal_write() {
        let mut terminal = Terminal::new_default().unwrap();
        terminal.write_str("Hello, World!\r\n");
        // Terminal should accept the data without crashing
    }

    #[test]
    fn test_terminal_resize() {
        let mut terminal = Terminal::new_default().unwrap();
        assert!(terminal.resize(100, 50).is_ok());
        let (cols, rows) = terminal.size();
        assert_eq!(cols, 100);
        assert_eq!(rows, 50);
    }

    #[test]
    fn test_terminal_cursor() {
        let mut terminal = Terminal::new_default().unwrap();
        terminal.write_str("ABCD");
        let (x, y) = terminal.cursor_pos();
        assert_eq!(x, 4);
        assert_eq!(y, 0);
    }

    #[test]
    fn test_screen_buffer() {
        let mut terminal = Terminal::new_default().unwrap();
        terminal.write_str("Hello");

        let buffer = terminal.read_screen();
        assert_eq!(buffer.rows, 24);
        assert_eq!(buffer.cols, 80);

        // Check that we can access cells
        let cell = terminal.cell_at(0, 0).unwrap();
        assert_eq!(cell.char, 'H');

        let cell = terminal.cell_at(0, 1).unwrap();
        assert_eq!(cell.char, 'e');
    }

    #[test]
    fn test_cursor_screen_pos() {
        let mut terminal = Terminal::new_default().unwrap();
        terminal.write_str("Test");

        let (x, y) = terminal.cursor_screen_pos();
        assert_eq!(x, 4);
        assert_eq!(y, 0);
    }

    #[test]
    fn test_read_screen_multiple_rows() {
        let mut terminal = Terminal::new_default().unwrap();

        // Write ABC on row 0, DEF on row 1
        terminal.write_str("ABC\r\nDEF");

        // Check cursor position
        let (x, y) = terminal.cursor_pos();
        assert_eq!(x, 3, "cursor x should be 3");
        assert_eq!(y, 1, "cursor y should be 1");

        // Row 0 should have ABC
        assert_eq!(terminal.cell_at(0, 0).unwrap().char, 'A');
        assert_eq!(terminal.cell_at(0, 1).unwrap().char, 'B');
        assert_eq!(terminal.cell_at(0, 2).unwrap().char, 'C');

        // Row 1 should have DEF
        assert_eq!(terminal.cell_at(1, 0).unwrap().char, 'D');
        assert_eq!(terminal.cell_at(1, 1).unwrap().char, 'E');
        assert_eq!(terminal.cell_at(1, 2).unwrap().char, 'F');
    }
}
