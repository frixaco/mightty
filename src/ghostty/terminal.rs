use std::ffi::c_void;
use std::marker::PhantomData;
use std::mem::{MaybeUninit, size_of};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::rc::Rc;

use crate::ghostty::error::{from_result, from_result_with_len};
use crate::ghostty::selection::{SelectionDrag, SelectionGesture, SelectionPress, SelectionUpdate};
use crate::ghostty::style::{Palette, RgbColor};
use crate::ghostty::{Error, Result, ffi};

/// An owned Ghostty terminal.
///
/// Ghostty objects are intentionally neither `Send` nor `Sync`. A terminal and
/// all render/input helpers that observe it must stay on their creating thread.
pub struct Terminal {
    raw: NonNull<ffi::TerminalImpl>,
    selection_gesture: SelectionGesture,
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

/// A terminal viewport movement in Ghostty's scrollback row space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewportScroll {
    Top,
    Bottom,
    Delta(isize),
    Row(usize),
}

/// Dimensions and current position of the terminal viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
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

        let raw = NonNull::new(raw).ok_or(Error::InvalidValue)?;
        let selection_gesture = match SelectionGesture::new() {
            Ok(gesture) => gesture,
            Err(error) => {
                unsafe {
                    ffi::ghostty_terminal_free(raw.as_ptr());
                }
                return Err(error);
            }
        };
        let mut terminal = Self {
            raw,
            selection_gesture,
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

    pub fn scroll_viewport(&mut self, scroll: ViewportScroll) {
        let (tag, value) = match scroll {
            ViewportScroll::Top => (
                ffi::TerminalScrollViewportTag::TOP,
                ffi::TerminalScrollViewportValue::default(),
            ),
            ViewportScroll::Bottom => (
                ffi::TerminalScrollViewportTag::BOTTOM,
                ffi::TerminalScrollViewportValue::default(),
            ),
            ViewportScroll::Delta(delta) => (
                ffi::TerminalScrollViewportTag::DELTA,
                ffi::TerminalScrollViewportValue { delta },
            ),
            ViewportScroll::Row(row) => (
                ffi::TerminalScrollViewportTag::ROW,
                ffi::TerminalScrollViewportValue { row },
            ),
        };
        unsafe {
            ffi::ghostty_terminal_scroll_viewport(
                self.as_raw(),
                ffi::TerminalScrollViewport { tag, value },
            );
        }
    }

    pub fn scrollbar(&self) -> Result<Scrollbar> {
        let raw =
            unsafe { self.get_unchecked::<ffi::TerminalScrollbar>(ffi::TerminalData::SCROLLBAR)? };
        Ok(Scrollbar {
            total: raw.total,
            offset: raw.offset,
            len: raw.len,
        })
    }

    pub fn selection_press(&mut self, input: SelectionPress) -> Result<()> {
        let update = self.selection_gesture.press(self.as_raw(), input)?;
        self.apply_selection_update(update)
    }

    pub fn selection_drag(&mut self, input: SelectionDrag) -> Result<()> {
        let update = self.selection_gesture.drag(self.as_raw(), input)?;
        self.apply_selection_update(update)
    }

    pub fn selection_release(
        &mut self,
        point: Option<crate::ghostty::SelectionPoint>,
    ) -> Result<()> {
        self.selection_gesture.release(self.as_raw(), point)
    }

    pub fn clear_selection(&mut self) -> Result<()> {
        self.set_raw_pointer(ffi::TerminalOption::SELECTION, std::ptr::null())
    }

    pub fn selected_text(&self) -> Result<Option<String>> {
        let options = ffi::TerminalSelectionFormatOptions {
            size: size_of::<ffi::TerminalSelectionFormatOptions>(),
            emit: ffi::FormatterFormat::PLAIN,
            unwrap: true,
            trim: true,
            selection: std::ptr::null(),
        };
        let mut required = 0_usize;
        let result = unsafe {
            ffi::ghostty_terminal_selection_format_buf(
                self.as_raw(),
                options,
                std::ptr::null_mut(),
                0,
                &raw mut required,
            )
        };
        match result {
            ffi::Result::NO_VALUE => return Ok(None),
            ffi::Result::SUCCESS if required == 0 => return Ok(Some(String::new())),
            ffi::Result::OUT_OF_SPACE => {}
            other => {
                from_result_with_len(other, required)?;
                return Err(Error::InvalidValue);
            }
        }

        let mut bytes = vec![0_u8; required];
        let mut written = 0_usize;
        let result = unsafe {
            ffi::ghostty_terminal_selection_format_buf(
                self.as_raw(),
                options,
                bytes.as_mut_ptr(),
                bytes.len(),
                &raw mut written,
            )
        };
        let written = from_result_with_len(result, written)?;
        if written > bytes.len() {
            return Err(Error::InvalidValue);
        }
        bytes.truncate(written);
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| Error::InvalidValue)
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

    fn apply_selection_update(&mut self, update: SelectionUpdate) -> Result<()> {
        match update {
            SelectionUpdate::Set(selection) => self.set_raw_pointer(
                ffi::TerminalOption::SELECTION,
                std::ptr::from_ref(&selection).cast(),
            ),
            SelectionUpdate::Clear => self.clear_selection(),
            SelectionUpdate::Unchanged => Ok(()),
        }
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

    /// `T` must be the output type documented for `data`.
    unsafe fn get_unchecked<T>(&self, data: ffi::TerminalData::Type) -> Result<T> {
        let mut value = MaybeUninit::<T>::uninit();
        let result =
            unsafe { ffi::ghostty_terminal_get(self.as_raw(), data, value.as_mut_ptr().cast()) };
        from_result(result)?;
        Ok(unsafe { value.assume_init() })
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.selection_gesture.deinit(self.raw.as_ptr());
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
    use crate::ghostty::{SelectionGeometry, SelectionPoint};

    const CELL_WIDTH: u32 = 10;
    const CELL_HEIGHT: u32 = 20;
    const REPEAT_INTERVAL_NS: u64 = 500_000_000;

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

    #[test]
    fn scrolls_viewport_in_ghostty_row_space() {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 12,
            rows: 2,
            max_scrollback: 100,
        })
        .unwrap();
        terminal.vt_write(b"one\r\ntwo\r\nthree\r\nfour");

        let at_bottom = terminal.scrollbar().unwrap();
        assert!(at_bottom.total > at_bottom.len);
        assert_eq!(at_bottom.offset, at_bottom.total - at_bottom.len);

        terminal.scroll_viewport(ViewportScroll::Delta(-1));

        let scrolled = terminal.scrollbar().unwrap();
        assert_eq!(scrolled.offset + 1, at_bottom.offset);
        assert_eq!(scrolled.total, at_bottom.total);
        assert_eq!(scrolled.len, at_bottom.len);
    }

    #[test]
    fn gesture_selects_and_formats_text_with_ghostty_rules() {
        let mut terminal = selection_terminal(20, 3);
        terminal.vt_write(b"hello world");

        terminal.selection_press(selection_press(0, 0, 1)).unwrap();
        terminal
            .selection_drag(selection_drag(4, 0, 20, 3))
            .unwrap();
        terminal
            .selection_release(Some(selection_point(4, 0)))
            .unwrap();

        assert_eq!(terminal.selected_text().unwrap().as_deref(), Some("hello"));
    }

    #[test]
    fn tracked_selection_survives_output_entering_scrollback() {
        let mut terminal = selection_terminal(12, 2);
        terminal.vt_write(b"one\r\ntwo");

        terminal.selection_press(selection_press(0, 0, 1)).unwrap();
        terminal
            .selection_drag(selection_drag(2, 0, 12, 2))
            .unwrap();
        terminal
            .selection_release(Some(selection_point(2, 0)))
            .unwrap();
        terminal.vt_write(b"\r\nthree");

        assert_eq!(terminal.selected_text().unwrap().as_deref(), Some("one"));
    }

    #[test]
    fn repeated_clicks_use_ghostty_word_and_line_selection() {
        let mut terminal = selection_terminal(20, 2);
        terminal.vt_write(b"hello world");
        let point = selection_point(1, 0);

        terminal.selection_press(selection_press(1, 0, 1)).unwrap();
        terminal.selection_release(Some(point)).unwrap();
        terminal
            .selection_press(selection_press(1, 0, 100_000_001))
            .unwrap();

        assert_eq!(terminal.selected_text().unwrap().as_deref(), Some("hello"));

        terminal.selection_release(Some(point)).unwrap();
        terminal
            .selection_press(selection_press(1, 0, 200_000_001))
            .unwrap();

        assert_eq!(
            terminal.selected_text().unwrap().as_deref(),
            Some("hello world")
        );
    }

    fn selection_terminal(cols: u16, rows: u16) -> Terminal {
        let mut terminal = Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: 100,
        })
        .unwrap();
        terminal
            .resize(cols, rows, CELL_WIDTH, CELL_HEIGHT)
            .unwrap();
        terminal
    }

    fn selection_press(column: u16, row: u32, time_ns: u64) -> SelectionPress {
        SelectionPress {
            point: selection_point(column, row),
            time_ns,
            repeat_interval_ns: REPEAT_INTERVAL_NS,
            repeat_distance: CELL_WIDTH as f64,
        }
    }

    fn selection_drag(column: u16, row: u32, columns: u32, rows: u32) -> SelectionDrag {
        let mut point = selection_point(column, row);
        point.surface_x += f64::from(CELL_WIDTH - 2);
        SelectionDrag {
            point,
            geometry: SelectionGeometry {
                columns,
                cell_width: CELL_WIDTH,
                screen_height: rows * CELL_HEIGHT,
            },
            rectangle: false,
        }
    }

    fn selection_point(column: u16, row: u32) -> SelectionPoint {
        SelectionPoint {
            column,
            row,
            surface_x: f64::from(column) * f64::from(CELL_WIDTH) + 1.0,
            surface_y: f64::from(row) * f64::from(CELL_HEIGHT) + 1.0,
        }
    }
}
