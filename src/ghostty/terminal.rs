use std::ffi::c_void;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::rc::Rc;

use crate::ghostty::error::from_result;
use crate::ghostty::style::{Palette, RgbColor};
use crate::ghostty::{Error, Result, ffi};

/// An owned Ghostty terminal.
///
/// Ghostty objects are intentionally neither `Send` nor `Sync`. A terminal and
/// all render/input helpers that observe it must stay on their creating thread.
pub struct Terminal {
    raw: NonNull<ffi::TerminalImpl>,
    callbacks: Box<CallbackState>,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

/// Terminal dimensions and scrollback limit used at creation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalOptions {
    pub cols: u16,
    pub rows: u16,
    pub max_scrollback: usize,
}

impl Terminal {
    pub fn new(options: TerminalOptions) -> Result<Self> {
        if options.cols == 0 || options.rows == 0 {
            return Err(Error::InvalidValue);
        }

        let mut raw = std::ptr::null_mut();
        let raw_options = ffi::TerminalOptions {
            cols: options.cols,
            rows: options.rows,
            max_scrollback: options.max_scrollback,
        };
        let result =
            unsafe { ffi::ghostty_terminal_new(std::ptr::null(), &raw mut raw, raw_options) };
        from_result(result)?;

        let mut terminal = Self {
            raw: NonNull::new(raw).ok_or(Error::InvalidValue)?,
            callbacks: Box::new(CallbackState::default()),
            _not_send_or_sync: PhantomData,
        };
        let userdata = std::ptr::from_mut(terminal.callbacks.as_mut()).cast::<c_void>();
        terminal.set_raw_pointer(ffi::TerminalOption::USERDATA, userdata)?;
        Ok(terminal)
    }

    pub fn vt_write(&mut self, data: &[u8]) {
        unsafe {
            ffi::ghostty_terminal_vt_write(self.as_raw(), data.as_ptr(), data.len());
        }
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<()> {
        if cols == 0 || rows == 0 {
            return Err(Error::InvalidValue);
        }
        let result = unsafe {
            ffi::ghostty_terminal_resize(self.as_raw(), cols, rows, cell_width_px, cell_height_px)
        };
        from_result(result)
    }

    /// Register the sink for terminal-generated replies sent back to the PTY.
    pub fn on_pty_write(&mut self, callback: impl FnMut(&[u8]) + 'static) -> Result<&mut Self> {
        self.callbacks.pty_write = Some(Box::new(callback));
        let callback: ffi::TerminalWritePtyFn = Some(pty_write_trampoline);
        let pointer = callback.map_or(std::ptr::null_mut(), |callback| {
            callback as *const () as *mut c_void
        });
        self.set_raw_pointer(ffi::TerminalOption::WRITE_PTY, pointer)?;
        Ok(self)
    }

    pub fn set_default_fg_color(&mut self, color: Option<RgbColor>) -> Result<&mut Self> {
        self.set_optional_color(ffi::TerminalOption::COLOR_FOREGROUND, color)?;
        Ok(self)
    }

    pub fn set_default_bg_color(&mut self, color: Option<RgbColor>) -> Result<&mut Self> {
        self.set_optional_color(ffi::TerminalOption::COLOR_BACKGROUND, color)?;
        Ok(self)
    }

    pub fn set_default_cursor_color(&mut self, color: Option<RgbColor>) -> Result<&mut Self> {
        self.set_optional_color(ffi::TerminalOption::COLOR_CURSOR, color)?;
        Ok(self)
    }

    pub fn set_default_color_palette(&mut self, palette: Option<Palette>) -> Result<&mut Self> {
        let raw = palette.map(Palette::into_raw);
        let pointer = raw
            .as_ref()
            .map_or(std::ptr::null(), |colors| colors.as_ptr().cast());
        self.set_raw_pointer(ffi::TerminalOption::COLOR_PALETTE, pointer)?;
        Ok(self)
    }

    pub(crate) fn as_raw(&self) -> ffi::Terminal {
        self.raw.as_ptr()
    }

    fn set_optional_color(
        &self,
        option: ffi::TerminalOption::Type,
        color: Option<RgbColor>,
    ) -> Result<()> {
        let raw = color.map(ffi::ColorRgb::from);
        let pointer = raw
            .as_ref()
            .map_or(std::ptr::null(), |value| std::ptr::from_ref(value).cast());
        self.set_raw_pointer(option, pointer)
    }

    fn set_raw_pointer(
        &self,
        option: ffi::TerminalOption::Type,
        pointer: *const c_void,
    ) -> Result<()> {
        let result = unsafe { ffi::ghostty_terminal_set(self.as_raw(), option, pointer) };
        from_result(result)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_terminal_free(self.as_raw());
        }
    }
}

type PtyWriteCallback = dyn FnMut(&[u8]) + 'static;

#[derive(Default)]
struct CallbackState {
    pty_write: Option<Box<PtyWriteCallback>>,
}

unsafe extern "C" fn pty_write_trampoline(
    _terminal: ffi::Terminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    let Some(callbacks) = NonNull::new(userdata.cast::<CallbackState>()) else {
        return;
    };
    if data.is_null() && len != 0 {
        return;
    }
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };

    let callback_result = catch_unwind(AssertUnwindSafe(|| {
        let callbacks = unsafe { callbacks.as_ptr().as_mut() }.expect("non-null callback state");
        if let Some(callback) = callbacks.pty_write.as_deref_mut() {
            callback(bytes);
        }
    }));
    if callback_result.is_err() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            eprintln!("panic in Ghostty PTY callback; response was dropped");
        }));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[test]
    fn rejects_zero_sized_terminal() {
        assert!(matches!(
            Terminal::new(TerminalOptions {
                cols: 0,
                rows: 24,
                max_scrollback: 0,
            }),
            Err(Error::InvalidValue)
        ));
    }

    #[test]
    fn delivers_terminal_responses_to_owned_callback() {
        let responses = Rc::new(RefCell::new(Vec::new()));
        let callback_responses = Rc::clone(&responses);
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 80,
            rows: 24,
            max_scrollback: 0,
        })
        .unwrap();
        terminal
            .on_pty_write(move |bytes| {
                callback_responses.borrow_mut().extend_from_slice(bytes);
            })
            .unwrap();

        terminal.vt_write(b"\x1b[5n");

        assert_eq!(responses.borrow().as_slice(), b"\x1b[0n");
    }

    #[test]
    fn contains_panics_from_terminal_callbacks() {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 80,
            rows: 24,
            max_scrollback: 0,
        })
        .unwrap();
        terminal
            .on_pty_write(|_| panic!("callback panic must not cross C"))
            .unwrap();

        terminal.vt_write(b"\x1b[5n");
    }
}
