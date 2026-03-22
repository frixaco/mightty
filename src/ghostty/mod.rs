//! Rust FFI bindings for libghostty-vt
//!
//! libghostty-vt is a virtual terminal emulator library that provides
//! functionality for parsing terminal escape sequences and maintaining
//! terminal state.

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::ffi::{c_int, c_void};
use std::ptr::NonNull;

// Re-export types
mod types;
pub use types::*;

/// Opaque handle to a terminal instance
#[derive(Debug)]
pub struct Terminal {
    ptr: NonNull<GhosttyTerminalInner>,
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
    }

    /// Write a string to the terminal
    pub fn write_str(&mut self, s: &str) {
        self.write(s.as_bytes());
    }

    /// Resize the terminal
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), GhosttyResult> {
        let result = unsafe { ghostty_terminal_resize(self.ptr.as_ptr(), cols, rows) };
        if result == GhosttyResult::Success {
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
        // After writing 4 characters, cursor should be at column 4 (0-indexed)
        assert_eq!(x, 4);
        assert_eq!(y, 0);
    }
}
