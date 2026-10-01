use std::ffi::c_void;
use std::marker::PhantomData;
use std::mem::{MaybeUninit, size_of};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::rc::Rc;

use crate::ghostty::error::{from_result, from_result_with_len};
use crate::ghostty::search::SearchState;
use crate::ghostty::selection::{SelectionDrag, SelectionGesture, SelectionPress, SelectionUpdate};
use crate::ghostty::style::{Palette, RgbColor};
use crate::ghostty::{Error, Result, ffi};

/// An owned Ghostty terminal.
///
/// Ghostty objects are intentionally neither `Send` nor `Sync`. A terminal and
/// all render/input helpers that observe it must stay on their creating thread.
pub struct Terminal {
    raw: NonNull<ffi::TerminalImpl>,
    pub(crate) search: Option<SearchState>,
    pub(crate) control_searches: std::collections::BTreeMap<u64, SearchState>,
    pub(crate) next_control_search: u64,
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

/// Clipboard destination requested by terminal output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardLocation {
    Standard,
    Selection,
    Primary,
}

/// One MIME representation in a terminal clipboard request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardContent {
    pub mime: String,
    pub data: Vec<u8>,
}

/// One atomic clipboard write requested by terminal output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardWrite {
    pub location: ClipboardLocation,
    pub contents: Vec<ClipboardContent>,
}

/// Result returned to Ghostty after a clipboard write request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardWriteResult {
    Success,
    Denied,
    Unsupported,
    Busy,
    InvalidData,
    IoError,
}

impl Terminal {
    pub fn new(options: TerminalOptions) -> Result<Self> {
        if options.cols == 0 || options.rows == 0 {
            return Err(Error::InvalidValue);
        }

        let mut raw = std::ptr::null_mut();
        let result = unsafe {
            ffi::ghostty_terminal_new(std::ptr::null(), &raw mut raw, options.cols, options.rows)
        };
        from_result(result)?;

        let raw = NonNull::new(raw).ok_or(Error::InvalidValue)?;
        let result = unsafe {
            ffi::ghostty_terminal_set(
                raw.as_ptr(),
                ffi::TerminalOption::SCROLLBACK_MAX_LINES,
                std::ptr::from_ref(&options.max_scrollback).cast(),
            )
        };
        if let Err(error) = from_result(result) {
            unsafe {
                ffi::ghostty_terminal_free(raw.as_ptr());
            }
            return Err(error);
        }
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
            search: None,
            control_searches: Default::default(),
            next_control_search: 1,
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

    pub fn mouse_tracking(&self) -> Result<bool> {
        unsafe { self.get_unchecked(ffi::TerminalData::MOUSE_TRACKING) }
    }

    pub fn bracketed_paste(&self) -> Result<bool> {
        self.mode(2004)
    }

    pub fn encode_paste(&self, data: &[u8]) -> Result<Vec<u8>> {
        crate::ghostty::paste::encode(data, self.bracketed_paste()?)
    }

    pub fn title(&self) -> Result<Option<String>> {
        terminal_string(self.as_raw(), ffi::TerminalData::TITLE)
    }

    pub fn working_directory(&self) -> Result<Option<String>> {
        terminal_string(self.as_raw(), ffi::TerminalData::PWD)
    }

    pub fn hyperlink_uri(&self, column: u16, row: u32) -> Result<Option<String>> {
        let point = ffi::Point {
            tag: ffi::PointTag::VIEWPORT,
            value: ffi::PointValue {
                coordinate: ffi::PointCoordinate { x: column, y: row },
            },
        };
        let mut grid_ref = ffi::GridRef {
            size: size_of::<ffi::GridRef>(),
            ..Default::default()
        };
        let result =
            unsafe { ffi::ghostty_terminal_grid_ref(self.as_raw(), point, &raw mut grid_ref) };
        from_result(result)?;

        let mut required = 0;
        let result = unsafe {
            ffi::ghostty_grid_ref_hyperlink_uri(
                &raw const grid_ref,
                std::ptr::null_mut(),
                0,
                &raw mut required,
            )
        };
        match result {
            ffi::Result::SUCCESS if required == 0 => return Ok(None),
            ffi::Result::OUT_OF_SPACE => {}
            other => {
                from_result_with_len(other, required)?;
                return Err(Error::InvalidValue);
            }
        }

        let mut bytes = vec![0_u8; required];
        let mut written = 0;
        let result = unsafe {
            ffi::ghostty_grid_ref_hyperlink_uri(
                &raw const grid_ref,
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
        self.format_selection_text(None)
    }
    pub fn selection_coordinates(&self) -> Result<Option<[(u16, u32); 2]>> {
        let mut selection = ffi::Selection::default();
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.as_raw(),
                ffi::TerminalData::SELECTION,
                std::ptr::from_mut(&mut selection).cast(),
            )
        };
        if result == ffi::Result::NO_VALUE {
            return Ok(None);
        }
        from_result(result)?;
        let mut points = [(0, 0); 2];
        for (index, reference) in [&selection.start, &selection.end].into_iter().enumerate() {
            let mut coordinate = ffi::PointCoordinate::default();
            from_result(unsafe {
                ffi::ghostty_terminal_point_from_grid_ref(
                    self.as_raw(),
                    reference,
                    ffi::PointTag::SCREEN,
                    &mut coordinate,
                )
            })?;
            points[index] = (coordinate.x, coordinate.y);
        }
        Ok(Some(points))
    }
    pub fn has_selection(&self) -> Result<bool> {
        let mut selection = ffi::Selection::default();
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.as_raw(),
                ffi::TerminalData::SELECTION,
                std::ptr::from_mut(&mut selection).cast(),
            )
        };
        if result == ffi::Result::NO_VALUE {
            return Ok(false);
        }
        from_result(result)?;
        Ok(true)
    }

    pub(super) fn format_selection_text(
        &self,
        selection: Option<&ffi::Selection>,
    ) -> Result<Option<String>> {
        let options = ffi::TerminalSelectionFormatOptions {
            size: size_of::<ffi::TerminalSelectionFormatOptions>(),
            emit: ffi::FormatterFormat::PLAIN,
            unwrap: true,
            trim: true,
            selection: selection.map_or(std::ptr::null(), std::ptr::from_ref),
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

    pub fn on_bell(&mut self, callback: impl FnMut() + 'static) -> Result<&mut Self> {
        self.callbacks.bell = Some(Box::new(callback));
        let callback: ffi::TerminalBellFn = Some(bell_trampoline);
        let pointer = callback.map_or(std::ptr::null(), |callback| {
            callback as *const () as *const c_void
        });
        self.set_raw_pointer(ffi::TerminalOption::BELL, pointer)?;
        Ok(self)
    }

    pub fn on_title_changed(
        &mut self,
        callback: impl FnMut(Result<Option<String>>) + 'static,
    ) -> Result<&mut Self> {
        self.callbacks.title_changed = Some(Box::new(callback));
        let callback: ffi::TerminalTitleChangedFn = Some(title_changed_trampoline);
        let pointer = callback.map_or(std::ptr::null(), |callback| {
            callback as *const () as *const c_void
        });
        self.set_raw_pointer(ffi::TerminalOption::TITLE_CHANGED, pointer)?;
        Ok(self)
    }

    pub fn on_working_directory_changed(
        &mut self,
        callback: impl FnMut(Result<Option<String>>) + 'static,
    ) -> Result<&mut Self> {
        self.callbacks.working_directory_changed = Some(Box::new(callback));
        let callback: ffi::TerminalPwdChangedFn = Some(working_directory_changed_trampoline);
        let pointer = callback.map_or(std::ptr::null(), |callback| {
            callback as *const () as *const c_void
        });
        self.set_raw_pointer(ffi::TerminalOption::PWD_CHANGED, pointer)?;
        Ok(self)
    }

    pub fn on_clipboard_write(
        &mut self,
        callback: impl FnMut(ClipboardWrite) -> ClipboardWriteResult + 'static,
    ) -> Result<&mut Self> {
        self.callbacks.clipboard_write = Some(Box::new(callback));
        let callback: ffi::TerminalClipboardWriteFn = Some(clipboard_write_trampoline);
        let pointer = callback.map_or(std::ptr::null(), |callback| {
            callback as *const () as *const c_void
        });
        self.set_raw_pointer(ffi::TerminalOption::CLIPBOARD_WRITE, pointer)?;
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

    fn mode(&self, mode: ffi::Mode) -> Result<bool> {
        let mut config = ffi::TerminalModeConfig { mode, value: false };
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.as_raw(),
                ffi::TerminalData::MODE,
                std::ptr::from_mut(&mut config).cast(),
            )
        };
        from_result(result)?;
        Ok(config.value)
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
        self.search.take();
        self.control_searches.clear();
        self.selection_gesture.deinit(self.raw.as_ptr());
        unsafe {
            ffi::ghostty_terminal_free(self.as_raw());
        }
    }
}

type PtyWriteCallback = dyn FnMut(&[u8]) + 'static;
type BellCallback = dyn FnMut() + 'static;
type MetadataCallback = dyn FnMut(Result<Option<String>>) + 'static;
type ClipboardWriteCallback = dyn FnMut(ClipboardWrite) -> ClipboardWriteResult + 'static;

#[derive(Default)]
struct CallbackState {
    pty_write: Option<Box<PtyWriteCallback>>,
    bell: Option<Box<BellCallback>>,
    title_changed: Option<Box<MetadataCallback>>,
    working_directory_changed: Option<Box<MetadataCallback>>,
    clipboard_write: Option<Box<ClipboardWriteCallback>>,
}

unsafe extern "C" fn pty_write_trampoline(
    _terminal: ffi::Terminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if data.is_null() && len != 0 {
        return;
    }
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };

    invoke_callback(userdata, |callbacks| {
        if let Some(callback) = callbacks.pty_write.as_deref_mut() {
            callback(bytes);
        }
    });
}

unsafe extern "C" fn bell_trampoline(_terminal: ffi::Terminal, userdata: *mut c_void) {
    invoke_callback(userdata, |callbacks| {
        if let Some(callback) = callbacks.bell.as_deref_mut() {
            callback();
        }
    });
}

unsafe extern "C" fn title_changed_trampoline(terminal: ffi::Terminal, userdata: *mut c_void) {
    let title = terminal_string(terminal, ffi::TerminalData::TITLE);
    invoke_callback(userdata, |callbacks| {
        if let Some(callback) = callbacks.title_changed.as_deref_mut() {
            callback(title);
        }
    });
}

unsafe extern "C" fn working_directory_changed_trampoline(
    terminal: ffi::Terminal,
    userdata: *mut c_void,
) {
    let working_directory = terminal_string(terminal, ffi::TerminalData::PWD);
    invoke_callback(userdata, |callbacks| {
        if let Some(callback) = callbacks.working_directory_changed.as_deref_mut() {
            callback(working_directory);
        }
    });
}

unsafe extern "C" fn clipboard_write_trampoline(
    _terminal: ffi::Terminal,
    userdata: *mut c_void,
    write: *const ffi::ClipboardWrite,
) -> ffi::ClipboardWriteResult::Type {
    let Ok(write) = clipboard_write(write) else {
        return ffi::ClipboardWriteResult::INVALID_DATA;
    };
    invoke_callback(userdata, |callbacks| {
        callbacks
            .clipboard_write
            .as_deref_mut()
            .map_or(ClipboardWriteResult::Denied, |callback| callback(write))
    })
    .unwrap_or(ClipboardWriteResult::Denied)
    .as_raw()
}

fn invoke_callback<R>(
    userdata: *mut c_void,
    callback: impl FnOnce(&mut CallbackState) -> R,
) -> Option<R> {
    let callbacks = NonNull::new(userdata.cast::<CallbackState>())?;
    let result = catch_unwind(AssertUnwindSafe(|| {
        let callbacks = unsafe { callbacks.as_ptr().as_mut() }.expect("non-null callback state");
        callback(callbacks)
    }));
    match result {
        Ok(value) => Some(value),
        Err(_) => {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                eprintln!("panic in Ghostty terminal callback; effect was dropped");
            }));
            None
        }
    }
}

fn terminal_string(
    terminal: ffi::Terminal,
    data: ffi::TerminalData::Type,
) -> Result<Option<String>> {
    let mut raw = MaybeUninit::<ffi::String>::uninit();
    let result = unsafe { ffi::ghostty_terminal_get(terminal, data, raw.as_mut_ptr().cast()) };
    from_result(result)?;
    owned_terminal_string(unsafe { raw.assume_init() })
}

fn owned_terminal_string(raw: ffi::String) -> Result<Option<String>> {
    if raw.len == 0 {
        return Ok(None);
    }
    if raw.ptr.is_null() {
        return Err(Error::InvalidValue);
    }
    let bytes = unsafe { std::slice::from_raw_parts(raw.ptr, raw.len) };
    std::str::from_utf8(bytes)
        .map(|value| Some(value.to_owned()))
        .map_err(|_| Error::InvalidValue)
}

fn clipboard_write(write: *const ffi::ClipboardWrite) -> Result<ClipboardWrite> {
    let write = unsafe { write.as_ref() }.ok_or(Error::InvalidValue)?;
    if write.size < size_of::<ffi::ClipboardWrite>() {
        return Err(Error::InvalidValue);
    }
    let location = ClipboardLocation::from_raw(write.location)?;
    let contents = if write.contents_len == 0 {
        &[][..]
    } else {
        if write.contents.is_null() {
            return Err(Error::InvalidValue);
        }
        unsafe { std::slice::from_raw_parts(write.contents, write.contents_len) }
    };
    let contents = contents
        .iter()
        .map(|content| {
            let mime = ffi_string(content.mime)?;
            let data = ffi_bytes(content.data)?;
            Ok(ClipboardContent { mime, data })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(ClipboardWrite { location, contents })
}

fn ffi_string(value: ffi::String) -> Result<String> {
    let bytes = ffi_bytes(value)?;
    String::from_utf8(bytes).map_err(|_| Error::InvalidValue)
}

fn ffi_bytes(value: ffi::String) -> Result<Vec<u8>> {
    if value.len == 0 {
        return Ok(Vec::new());
    }
    if value.ptr.is_null() {
        return Err(Error::InvalidValue);
    }
    Ok(unsafe { std::slice::from_raw_parts(value.ptr, value.len) }.to_vec())
}

impl ClipboardLocation {
    fn from_raw(value: ffi::ClipboardLocation::Type) -> Result<Self> {
        match value {
            ffi::ClipboardLocation::STANDARD => Ok(Self::Standard),
            ffi::ClipboardLocation::SELECTION => Ok(Self::Selection),
            ffi::ClipboardLocation::PRIMARY => Ok(Self::Primary),
            _ => Err(Error::InvalidValue),
        }
    }
}

impl ClipboardWriteResult {
    fn as_raw(self) -> ffi::ClipboardWriteResult::Type {
        match self {
            Self::Success => ffi::ClipboardWriteResult::SUCCESS,
            Self::Denied => ffi::ClipboardWriteResult::DENIED,
            Self::Unsupported => ffi::ClipboardWriteResult::UNSUPPORTED,
            Self::Busy => ffi::ClipboardWriteResult::BUSY,
            Self::InvalidData => ffi::ClipboardWriteResult::INVALID_DATA,
            Self::IoError => ffi::ClipboardWriteResult::IO_ERROR,
        }
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
    fn exposes_mouse_and_bracketed_paste_modes() {
        let mut terminal = selection_terminal(80, 24);
        assert!(!terminal.mouse_tracking().unwrap());
        assert!(!terminal.bracketed_paste().unwrap());

        terminal.vt_write(b"\x1b[?1000h\x1b[?2004h");

        assert!(terminal.mouse_tracking().unwrap());
        assert!(terminal.bracketed_paste().unwrap());
        assert_eq!(
            terminal.encode_paste(b"one\ntwo").unwrap(),
            b"\x1b[200~one\ntwo\x1b[201~"
        );
    }

    #[test]
    fn reports_title_directory_bell_and_clipboard_effects() {
        let titles = Rc::new(RefCell::new(Vec::new()));
        let title_events = Rc::clone(&titles);
        let directories = Rc::new(RefCell::new(Vec::new()));
        let directory_events = Rc::clone(&directories);
        let bells = Rc::new(RefCell::new(0_u8));
        let bell_events = Rc::clone(&bells);
        let clipboard = Rc::new(RefCell::new(Vec::new()));
        let clipboard_events = Rc::clone(&clipboard);
        let mut terminal = selection_terminal(80, 24);
        terminal
            .on_title_changed(move |title| title_events.borrow_mut().push(title))
            .unwrap()
            .on_working_directory_changed(move |directory| {
                directory_events.borrow_mut().push(directory)
            })
            .unwrap()
            .on_bell(move || *bell_events.borrow_mut() += 1)
            .unwrap()
            .on_clipboard_write(move |write| {
                clipboard_events.borrow_mut().push(write);
                ClipboardWriteResult::Success
            })
            .unwrap();

        terminal
            .vt_write(b"\x1b]2;mightty test\x07\x1b]7;file:///tmp\x07\x07\x1b]52;c;aGVsbG8=\x07");

        assert_eq!(terminal.title().unwrap().as_deref(), Some("mightty test"));
        assert_eq!(
            titles.borrow().as_slice(),
            &[Ok(Some("mightty test".to_string()))]
        );
        assert_eq!(
            directories.borrow().last(),
            Some(&terminal.working_directory())
        );
        assert_eq!(*bells.borrow(), 1);
        assert_eq!(clipboard.borrow()[0].contents[0].data.as_slice(), b"hello");
    }

    #[test]
    fn ignores_invalid_utf8_metadata_and_accepts_the_next_valid_value() {
        let titles = Rc::new(RefCell::new(Vec::new()));
        let title_events = Rc::clone(&titles);
        let mut terminal = selection_terminal(80, 24);
        terminal
            .on_title_changed(move |title| title_events.borrow_mut().push(title))
            .unwrap();

        terminal.vt_write(b"\x1b]2;first\x07\x1b]2;\xff\x07\x1b]2;second\x07");

        assert_eq!(
            titles.borrow().as_slice(),
            &[
                Ok(Some("first".to_string())),
                Ok(Some("second".to_string()))
            ]
        );
    }

    #[test]
    fn rejects_invalid_utf8_from_ghostty_metadata() {
        let bytes = [0xff];
        assert_eq!(
            owned_terminal_string(ffi::String {
                ptr: bytes.as_ptr(),
                len: bytes.len(),
            }),
            Err(Error::InvalidValue)
        );
    }

    #[test]
    fn resolves_hyperlinks_at_viewport_cells() {
        let mut terminal = selection_terminal(20, 2);
        terminal.vt_write(b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\");

        assert_eq!(
            terminal.hyperlink_uri(0, 0).unwrap().as_deref(),
            Some("https://example.com")
        );
        assert_eq!(terminal.hyperlink_uri(5, 0).unwrap(), None);
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
