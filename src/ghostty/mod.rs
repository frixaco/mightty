//! Minimal Rust wrapper for the subset of libghostty-vt used by mightty.

use std::ffi::{c_char, c_void};
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::mpsc::Sender;

pub mod key {
    pub use super::{Action, Encoder, Event, Key, Mods};
}

pub mod render {
    pub use super::{CellIterator, CellWidth, RowIterator};
}

pub mod style {
    pub use super::{RgbColor, Underline};
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    OutOfMemory,
    InvalidValue,
    OutOfSpace,
    NoValue,
}

impl From<GhosttyResult> for Error {
    fn from(value: GhosttyResult) -> Self {
        match value {
            GhosttyResult::OutOfMemory => Self::OutOfMemory,
            GhosttyResult::InvalidValue => Self::InvalidValue,
            GhosttyResult::OutOfSpace => Self::OutOfSpace,
            GhosttyResult::NoValue => Self::NoValue,
            GhosttyResult::Success => Self::InvalidValue,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TerminalOptions {
    pub cols: u16,
    pub rows: u16,
    pub max_scrollback: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorViewport {
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct Colors {
    pub foreground: RgbColor,
    pub background: RgbColor,
    pub cursor: Option<RgbColor>,
    #[allow(dead_code)]
    pub palette: [RgbColor; 256],
}

#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: Underline,
    #[allow(dead_code)]
    pub blink: bool,
    pub inverse: bool,
    pub strikethrough: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellWidth {
    Narrow,
    Wide,
    SpacerTail,
    SpacerHead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Underline {
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Action {
    Release = 0,
    Press = 1,
    Repeat = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Key {
    Unidentified = 0,
    Backquote = 1,
    Backslash = 2,
    BracketLeft = 3,
    BracketRight = 4,
    Comma = 5,
    Digit0 = 6,
    Digit1 = 7,
    Digit2 = 8,
    Digit3 = 9,
    Digit4 = 10,
    Digit5 = 11,
    Digit6 = 12,
    Digit7 = 13,
    Digit8 = 14,
    Digit9 = 15,
    Equal = 16,
    A = 20,
    B = 21,
    C = 22,
    D = 23,
    E = 24,
    F = 25,
    G = 26,
    H = 27,
    I = 28,
    J = 29,
    K = 30,
    L = 31,
    M = 32,
    N = 33,
    O = 34,
    P = 35,
    Q = 36,
    R = 37,
    S = 38,
    T = 39,
    U = 40,
    V = 41,
    W = 42,
    X = 43,
    Y = 44,
    Z = 45,
    Minus = 46,
    Period = 47,
    Quote = 48,
    Semicolon = 49,
    Slash = 50,
    Backspace = 53,
    Enter = 58,
    Space = 63,
    Tab = 64,
    Delete = 68,
    End = 69,
    Home = 71,
    Insert = 72,
    PageDown = 73,
    PageUp = 74,
    ArrowDown = 75,
    ArrowLeft = 76,
    ArrowRight = 77,
    ArrowUp = 78,
    Escape = 120,
    F1 = 121,
    F2 = 122,
    F3 = 123,
    F4 = 124,
    F5 = 125,
    F6 = 126,
    F7 = 127,
    F8 = 128,
    F9 = 129,
    F10 = 130,
    F11 = 131,
    F12 = 132,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct Mods(u16);

impl Mods {
    pub const SHIFT: Self = Self(1 << 0);
    pub const CTRL: Self = Self(1 << 1);
    pub const ALT: Self = Self(1 << 2);
    pub const SUPER: Self = Self(1 << 3);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }
}

impl std::ops::BitOr for Mods {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Mods {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

pub struct Terminal<'a, 'b> {
    raw: GhosttyTerminal,
    effects: Box<TerminalEffects>,
    _marker: PhantomData<(&'a (), &'b ())>,
}

#[derive(Default)]
struct TerminalEffects {
    pty_response_tx: Option<Sender<Vec<u8>>>,
}

pub struct RenderState<'a> {
    raw: GhosttyRenderState,
    _marker: PhantomData<&'a ()>,
}

pub struct RowIterator<'a> {
    raw: GhosttyRenderStateRowIterator,
    _marker: PhantomData<&'a ()>,
}

pub struct CellIterator<'a> {
    raw: GhosttyRenderStateRowCells,
    _marker: PhantomData<&'a ()>,
}

pub struct Encoder<'a> {
    raw: GhosttyKeyEncoder,
    _marker: PhantomData<&'a ()>,
}

pub struct Event<'a> {
    raw: GhosttyKeyEvent,
    _marker: PhantomData<&'a ()>,
}

pub struct Snapshot<'a> {
    raw: GhosttyRenderState,
    _marker: PhantomData<&'a RenderState<'a>>,
}

pub struct Rows<'a> {
    raw: GhosttyRenderStateRowIterator,
    _marker: PhantomData<&'a mut GhosttyRenderStateRowIterator>,
}

#[derive(Clone, Copy)]
pub struct Row<'a> {
    raw: GhosttyRenderStateRowIterator,
    _marker: PhantomData<&'a GhosttyRenderStateRowIterator>,
}

#[derive(Clone, Copy)]
pub struct Cells<'a> {
    raw: GhosttyRenderStateRowCells,
    _marker: PhantomData<&'a mut GhosttyRenderStateRowCells>,
}

#[derive(Clone, Copy)]
pub struct Cell<'a> {
    raw: GhosttyRenderStateRowCells,
    _marker: PhantomData<&'a GhosttyRenderStateRowCells>,
}

impl<'a, 'b> Terminal<'a, 'b> {
    pub fn new(options: TerminalOptions) -> Result<Self> {
        let mut raw = ptr::null_mut();
        let result = unsafe {
            ghostty_terminal_new(
                ptr::null(),
                &mut raw,
                GhosttyTerminalOptions {
                    cols: options.cols,
                    rows: options.rows,
                    max_scrollback: options.max_scrollback,
                },
            )
        };
        check_result(result)?;

        let mut terminal = Self {
            raw,
            effects: Box::new(TerminalEffects::default()),
            _marker: PhantomData,
        };
        terminal.install_effects()?;

        Ok(terminal)
    }

    pub fn vt_write(&mut self, data: &[u8]) {
        unsafe {
            ghostty_terminal_vt_write(self.raw, data.as_ptr(), data.len());
        }
    }

    pub fn set_pty_response_sender(&mut self, tx: Sender<Vec<u8>>) -> Result<()> {
        self.effects.pty_response_tx = Some(tx);
        self.install_effects()
    }

    pub fn set_default_colors(
        &mut self,
        foreground: RgbColor,
        background: RgbColor,
        cursor: RgbColor,
        palette: &[RgbColor; 256],
    ) -> Result<()> {
        check_result(unsafe {
            ghostty_terminal_set(
                self.raw,
                GhosttyTerminalOption::ColorForeground,
                (&foreground as *const RgbColor).cast(),
            )
        })?;
        check_result(unsafe {
            ghostty_terminal_set(
                self.raw,
                GhosttyTerminalOption::ColorBackground,
                (&background as *const RgbColor).cast(),
            )
        })?;
        check_result(unsafe {
            ghostty_terminal_set(
                self.raw,
                GhosttyTerminalOption::ColorCursor,
                (&cursor as *const RgbColor).cast(),
            )
        })?;
        check_result(unsafe {
            ghostty_terminal_set(
                self.raw,
                GhosttyTerminalOption::ColorPalette,
                palette.as_ptr().cast(),
            )
        })
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<()> {
        let result =
            unsafe { ghostty_terminal_resize(self.raw, cols, rows, cell_width_px, cell_height_px) };
        check_result(result)
    }

    fn install_effects(&mut self) -> Result<()> {
        let userdata = self.effects.as_ref() as *const TerminalEffects as *const c_void;
        check_result(unsafe {
            ghostty_terminal_set(self.raw, GhosttyTerminalOption::Userdata, userdata)
        })?;

        let callback = terminal_write_pty_callback as GhosttyTerminalWritePtyFn;
        check_result(unsafe {
            ghostty_terminal_set(
                self.raw,
                GhosttyTerminalOption::WritePty,
                callback as *const () as *const c_void,
            )
        })
    }
}

impl<'a, 'b> Drop for Terminal<'a, 'b> {
    fn drop(&mut self) {
        unsafe {
            ghostty_terminal_free(self.raw);
        }
    }
}

impl<'a> RenderState<'a> {
    pub fn new() -> Result<Self> {
        let mut raw = ptr::null_mut();
        let result = unsafe { ghostty_render_state_new(ptr::null(), &mut raw) };
        check_result(result)?;

        Ok(Self {
            raw,
            _marker: PhantomData,
        })
    }

    pub fn update<'s, 't, 'u>(&'s mut self, terminal: &Terminal<'t, 'u>) -> Result<Snapshot<'s>> {
        let result = unsafe { ghostty_render_state_update(self.raw, terminal.raw) };
        check_result(result)?;

        Ok(Snapshot {
            raw: self.raw,
            _marker: PhantomData,
        })
    }
}

impl<'a> Drop for RenderState<'a> {
    fn drop(&mut self) {
        unsafe {
            ghostty_render_state_free(self.raw);
        }
    }
}

impl<'a> Snapshot<'a> {
    pub fn colors(&self) -> Result<Colors> {
        let mut raw = GhosttyRenderStateColors {
            size: mem::size_of::<GhosttyRenderStateColors>(),
            background: RgbColor { r: 0, g: 0, b: 0 },
            foreground: RgbColor { r: 0, g: 0, b: 0 },
            cursor: RgbColor { r: 0, g: 0, b: 0 },
            cursor_has_value: false,
            palette: [RgbColor { r: 0, g: 0, b: 0 }; 256],
        };
        let result = unsafe { ghostty_render_state_colors_get(self.raw, &mut raw) };
        check_result(result)?;

        Ok(Colors {
            foreground: raw.foreground,
            background: raw.background,
            cursor: raw.cursor_has_value.then_some(raw.cursor),
            palette: raw.palette,
        })
    }

    pub fn cursor_viewport(&self) -> Result<Option<CursorViewport>> {
        let mut has_value = false;
        check_result(unsafe {
            ghostty_render_state_get(
                self.raw,
                GhosttyRenderStateData::CursorViewportHasValue,
                (&mut has_value as *mut bool).cast(),
            )
        })?;

        if !has_value {
            return Ok(None);
        }

        let mut x = 0u16;
        let mut y = 0u16;
        check_result(unsafe {
            ghostty_render_state_get(
                self.raw,
                GhosttyRenderStateData::CursorViewportX,
                (&mut x as *mut u16).cast(),
            )
        })?;
        check_result(unsafe {
            ghostty_render_state_get(
                self.raw,
                GhosttyRenderStateData::CursorViewportY,
                (&mut y as *mut u16).cast(),
            )
        })?;

        Ok(Some(CursorViewport { x, y }))
    }
}

impl<'a> RowIterator<'a> {
    pub fn new() -> Result<Self> {
        let mut raw = ptr::null_mut();
        let result = unsafe { ghostty_render_state_row_iterator_new(ptr::null(), &mut raw) };
        check_result(result)?;

        Ok(Self {
            raw,
            _marker: PhantomData,
        })
    }

    pub fn update<'s>(&'s mut self, snapshot: &'s Snapshot<'_>) -> Result<Rows<'s>> {
        check_result(unsafe {
            ghostty_render_state_get(
                snapshot.raw,
                GhosttyRenderStateData::RowIterator,
                (&mut self.raw as *mut GhosttyRenderStateRowIterator).cast(),
            )
        })?;

        Ok(Rows {
            raw: self.raw,
            _marker: PhantomData,
        })
    }
}

impl<'a> Drop for RowIterator<'a> {
    fn drop(&mut self) {
        unsafe {
            ghostty_render_state_row_iterator_free(self.raw);
        }
    }
}

impl<'a> Rows<'a> {
    pub fn next(&mut self) -> Option<Row<'_>> {
        if unsafe { ghostty_render_state_row_iterator_next(self.raw) } {
            Some(Row {
                raw: self.raw,
                _marker: PhantomData,
            })
        } else {
            None
        }
    }
}

impl<'a> Row<'a> {
    pub fn set_dirty(&self, dirty: bool) -> Result<()> {
        check_result(unsafe {
            ghostty_render_state_row_set(
                self.raw,
                GhosttyRenderStateRowOption::Dirty,
                (&dirty as *const bool).cast(),
            )
        })
    }
}

impl<'a> CellIterator<'a> {
    pub fn new() -> Result<Self> {
        let mut raw = ptr::null_mut();
        let result = unsafe { ghostty_render_state_row_cells_new(ptr::null(), &mut raw) };
        check_result(result)?;

        Ok(Self {
            raw,
            _marker: PhantomData,
        })
    }

    pub fn update<'s>(&'s mut self, row: Row<'s>) -> Result<Cells<'s>> {
        check_result(unsafe {
            ghostty_render_state_row_get(
                row.raw,
                GhosttyRenderStateRowData::Cells,
                (&mut self.raw as *mut GhosttyRenderStateRowCells).cast(),
            )
        })?;

        Ok(Cells {
            raw: self.raw,
            _marker: PhantomData,
        })
    }
}

impl<'a> Drop for CellIterator<'a> {
    fn drop(&mut self) {
        unsafe {
            ghostty_render_state_row_cells_free(self.raw);
        }
    }
}

impl<'a> Cells<'a> {
    pub fn next(&mut self) -> Option<Cell<'_>> {
        if unsafe { ghostty_render_state_row_cells_next(self.raw) } {
            Some(Cell {
                raw: self.raw,
                _marker: PhantomData,
            })
        } else {
            None
        }
    }
}

impl<'a> Cell<'a> {
    pub fn width(&self) -> Result<CellWidth> {
        let raw = self.raw_cell()?;
        let mut width = GhosttyCellWide::Narrow;
        check_result(unsafe {
            ghostty_cell_get(
                raw,
                GhosttyCellData::Wide,
                (&mut width as *mut GhosttyCellWide).cast(),
            )
        })?;
        Ok(CellWidth::from_raw(width))
    }

    pub fn graphemes_len(&self) -> Result<u32> {
        let mut len = 0u32;
        check_result(unsafe {
            ghostty_render_state_row_cells_get(
                self.raw,
                GhosttyRenderStateRowCellsData::GraphemesLen,
                (&mut len as *mut u32).cast(),
            )
        })?;
        Ok(len)
    }

    pub fn graphemes(&self) -> Result<Vec<char>> {
        let len = self.graphemes_len()? as usize;
        if len == 0 {
            return Ok(Vec::new());
        }

        let mut buf = vec![0u32; len];
        check_result(unsafe {
            ghostty_render_state_row_cells_get(
                self.raw,
                GhosttyRenderStateRowCellsData::GraphemesBuf,
                buf.as_mut_ptr().cast(),
            )
        })?;

        Ok(buf.into_iter().filter_map(char::from_u32).collect())
    }

    pub fn fg_color(&self) -> Result<Option<RgbColor>> {
        get_optional_color(self.raw, GhosttyRenderStateRowCellsData::FgColor)
    }

    pub fn bg_color(&self) -> Result<Option<RgbColor>> {
        get_optional_color(self.raw, GhosttyRenderStateRowCellsData::BgColor)
    }

    pub fn style(&self) -> Result<Style> {
        let mut raw = GhosttyStyle {
            size: mem::size_of::<GhosttyStyle>(),
            fg_color: GhosttyStyleColor::default(),
            bg_color: GhosttyStyleColor::default(),
            underline_color: GhosttyStyleColor::default(),
            bold: false,
            italic: false,
            faint: false,
            blink: false,
            inverse: false,
            invisible: false,
            strikethrough: false,
            overline: false,
            underline: 0,
        };
        check_result(unsafe {
            ghostty_render_state_row_cells_get(
                self.raw,
                GhosttyRenderStateRowCellsData::Style,
                (&mut raw as *mut GhosttyStyle).cast(),
            )
        })?;

        Ok(Style {
            bold: raw.bold,
            italic: raw.italic,
            underline: Underline::from_raw(raw.underline),
            blink: raw.blink,
            inverse: raw.inverse,
            strikethrough: raw.strikethrough,
        })
    }

    fn raw_cell(&self) -> Result<GhosttyCell> {
        let mut raw = 0u64;
        check_result(unsafe {
            ghostty_render_state_row_cells_get(
                self.raw,
                GhosttyRenderStateRowCellsData::Raw,
                (&mut raw as *mut GhosttyCell).cast(),
            )
        })?;
        Ok(raw)
    }
}

impl<'a> Encoder<'a> {
    pub fn new() -> Result<Self> {
        let mut raw = ptr::null_mut();
        let result = unsafe { ghostty_key_encoder_new(ptr::null(), &mut raw) };
        check_result(result)?;

        Ok(Self {
            raw,
            _marker: PhantomData,
        })
    }

    pub fn set_options_from_terminal<'t, 'u>(&mut self, terminal: &Terminal<'t, 'u>) {
        unsafe {
            ghostty_key_encoder_setopt_from_terminal(self.raw, terminal.raw);
        }
    }

    pub fn encode_to_vec(&mut self, event: &Event<'_>, out: &mut Vec<u8>) -> Result<()> {
        let mut required = 0usize;
        let result = unsafe {
            ghostty_key_encoder_encode(self.raw, event.raw, ptr::null_mut(), 0, &mut required)
        };

        match result {
            GhosttyResult::Success => {
                out.clear();
                return Ok(());
            }
            GhosttyResult::OutOfSpace => {}
            other => return Err(other.into()),
        }

        let mut buf = vec![0u8; required];
        check_result(unsafe {
            ghostty_key_encoder_encode(
                self.raw,
                event.raw,
                buf.as_mut_ptr().cast::<c_char>(),
                buf.len(),
                &mut required,
            )
        })?;
        buf.truncate(required);

        out.clear();
        out.extend_from_slice(&buf);
        Ok(())
    }
}

impl<'a> Drop for Encoder<'a> {
    fn drop(&mut self) {
        unsafe {
            ghostty_key_encoder_free(self.raw);
        }
    }
}

impl<'a> Event<'a> {
    pub fn new() -> Result<Self> {
        let mut raw = ptr::null_mut();
        let result = unsafe { ghostty_key_event_new(ptr::null(), &mut raw) };
        check_result(result)?;

        Ok(Self {
            raw,
            _marker: PhantomData,
        })
    }

    pub fn set_action(&mut self, action: Action) -> &mut Self {
        unsafe {
            ghostty_key_event_set_action(self.raw, action);
        }
        self
    }

    pub fn set_key(&mut self, key: Key) -> &mut Self {
        unsafe {
            ghostty_key_event_set_key(self.raw, key);
        }
        self
    }

    pub fn set_mods(&mut self, mods: Mods) -> &mut Self {
        unsafe {
            ghostty_key_event_set_mods(self.raw, mods.0);
        }
        self
    }

    pub fn set_consumed_mods(&mut self, mods: Mods) -> &mut Self {
        unsafe {
            ghostty_key_event_set_consumed_mods(self.raw, mods.0);
        }
        self
    }

    pub fn set_unshifted_codepoint(&mut self, codepoint: char) -> &mut Self {
        unsafe {
            ghostty_key_event_set_unshifted_codepoint(self.raw, codepoint as u32);
        }
        self
    }

    pub fn set_utf8(&mut self, text: Option<&str>) -> &mut Self {
        match text {
            Some(text) => unsafe {
                ghostty_key_event_set_utf8(self.raw, text.as_ptr().cast(), text.len());
            },
            None => unsafe {
                ghostty_key_event_set_utf8(self.raw, ptr::null(), 0);
            },
        }
        self
    }

    pub fn set_composing(&mut self, composing: bool) -> &mut Self {
        unsafe {
            ghostty_key_event_set_composing(self.raw, composing);
        }
        self
    }
}

impl<'a> Drop for Event<'a> {
    fn drop(&mut self) {
        unsafe {
            ghostty_key_event_free(self.raw);
        }
    }
}

impl Underline {
    fn from_raw(value: i32) -> Self {
        match value {
            1 => Self::Single,
            2 => Self::Double,
            3 => Self::Curly,
            4 => Self::Dotted,
            5 => Self::Dashed,
            _ => Self::None,
        }
    }
}

impl CellWidth {
    pub fn column_advance(self) -> u16 {
        match self {
            Self::Narrow => 1,
            Self::Wide => 2,
            Self::SpacerTail | Self::SpacerHead => 0,
        }
    }

    fn from_raw(value: GhosttyCellWide) -> Self {
        match value {
            GhosttyCellWide::Wide => Self::Wide,
            GhosttyCellWide::SpacerTail => Self::SpacerTail,
            GhosttyCellWide::SpacerHead => Self::SpacerHead,
            GhosttyCellWide::Narrow => Self::Narrow,
        }
    }
}

fn get_optional_color(
    raw: GhosttyRenderStateRowCells,
    data: GhosttyRenderStateRowCellsData,
) -> Result<Option<RgbColor>> {
    let mut color = RgbColor { r: 0, g: 0, b: 0 };
    match unsafe {
        ghostty_render_state_row_cells_get(raw, data, (&mut color as *mut RgbColor).cast())
    } {
        GhosttyResult::Success => Ok(Some(color)),
        GhosttyResult::NoValue | GhosttyResult::InvalidValue => Ok(None),
        other => Err(other.into()),
    }
}

fn check_result(result: GhosttyResult) -> Result<()> {
    match result {
        GhosttyResult::Success => Ok(()),
        other => Err(other.into()),
    }
}

#[allow(dead_code)]
type GhosttyTerminal = *mut c_void;
#[allow(dead_code)]
type GhosttyRenderState = *mut c_void;
#[allow(dead_code)]
type GhosttyRenderStateRowIterator = *mut c_void;
#[allow(dead_code)]
type GhosttyRenderStateRowCells = *mut c_void;
#[allow(dead_code)]
type GhosttyKeyEncoder = *mut c_void;
#[allow(dead_code)]
type GhosttyKeyEvent = *mut c_void;
type GhosttyCell = u64;
type GhosttyTerminalWritePtyFn = extern "C" fn(GhosttyTerminal, *mut c_void, *const u8, usize);

#[derive(Clone, Copy)]
#[repr(C)]
struct GhosttyTerminalOptions {
    cols: u16,
    rows: u16,
    max_scrollback: usize,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
#[repr(i32)]
enum GhosttyResult {
    Success = 0,
    OutOfMemory = -1,
    InvalidValue = -2,
    OutOfSpace = -3,
    NoValue = -4,
}

#[derive(Clone, Copy)]
#[repr(i32)]
enum GhosttyTerminalOption {
    Userdata = 0,
    WritePty = 1,
    ColorForeground = 11,
    ColorBackground = 12,
    ColorCursor = 13,
    ColorPalette = 14,
}

#[derive(Clone, Copy)]
#[repr(i32)]
enum GhosttyRenderStateData {
    RowIterator = 4,
    CursorViewportHasValue = 14,
    CursorViewportX = 15,
    CursorViewportY = 16,
}

#[derive(Clone, Copy)]
#[repr(i32)]
enum GhosttyRenderStateRowData {
    Cells = 3,
}

#[derive(Clone, Copy)]
#[repr(i32)]
enum GhosttyRenderStateRowOption {
    Dirty = 0,
}

#[derive(Clone, Copy)]
#[repr(i32)]
enum GhosttyRenderStateRowCellsData {
    Raw = 1,
    Style = 2,
    GraphemesLen = 3,
    GraphemesBuf = 4,
    BgColor = 5,
    FgColor = 6,
}

#[derive(Clone, Copy)]
#[repr(i32)]
enum GhosttyCellData {
    Wide = 3,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
#[repr(i32)]
enum GhosttyCellWide {
    Narrow = 0,
    Wide = 1,
    SpacerTail = 2,
    SpacerHead = 3,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct GhosttyRenderStateColors {
    size: usize,
    background: RgbColor,
    foreground: RgbColor,
    cursor: RgbColor,
    cursor_has_value: bool,
    palette: [RgbColor; 256],
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct GhosttyStyleColor {
    tag: i32,
    value: GhosttyStyleColorValue,
}

#[derive(Clone, Copy)]
#[repr(C)]
union GhosttyStyleColorValue {
    palette: u8,
    rgb: RgbColor,
    padding: u64,
}

impl Default for GhosttyStyleColorValue {
    fn default() -> Self {
        Self { padding: 0 }
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
struct GhosttyStyle {
    size: usize,
    fg_color: GhosttyStyleColor,
    bg_color: GhosttyStyleColor,
    underline_color: GhosttyStyleColor,
    bold: bool,
    italic: bool,
    faint: bool,
    blink: bool,
    inverse: bool,
    invisible: bool,
    strikethrough: bool,
    overline: bool,
    underline: i32,
}

extern "C" fn terminal_write_pty_callback(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if userdata.is_null() || data.is_null() || len == 0 {
        return;
    }

    let effects = unsafe { &*(userdata as *const TerminalEffects) };
    let Some(tx) = &effects.pty_response_tx else {
        return;
    };

    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    let _ = tx.send(bytes);
}

#[cfg_attr(windows, link(name = "ghostty-vt", kind = "raw-dylib"))]
unsafe extern "C" {
    fn ghostty_terminal_new(
        allocator: *const c_void,
        terminal: *mut GhosttyTerminal,
        options: GhosttyTerminalOptions,
    ) -> GhosttyResult;
    fn ghostty_terminal_free(terminal: GhosttyTerminal);
    fn ghostty_terminal_resize(
        terminal: GhosttyTerminal,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> GhosttyResult;
    fn ghostty_terminal_set(
        terminal: GhosttyTerminal,
        option: GhosttyTerminalOption,
        value: *const c_void,
    ) -> GhosttyResult;
    fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);

    fn ghostty_render_state_new(
        allocator: *const c_void,
        state: *mut GhosttyRenderState,
    ) -> GhosttyResult;
    fn ghostty_render_state_free(state: GhosttyRenderState);
    fn ghostty_render_state_update(
        state: GhosttyRenderState,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    fn ghostty_render_state_get(
        state: GhosttyRenderState,
        data: GhosttyRenderStateData,
        out: *mut c_void,
    ) -> GhosttyResult;
    fn ghostty_render_state_colors_get(
        state: GhosttyRenderState,
        out_colors: *mut GhosttyRenderStateColors,
    ) -> GhosttyResult;
    fn ghostty_render_state_row_iterator_new(
        allocator: *const c_void,
        out_iterator: *mut GhosttyRenderStateRowIterator,
    ) -> GhosttyResult;
    fn ghostty_render_state_row_iterator_free(iterator: GhosttyRenderStateRowIterator);
    fn ghostty_render_state_row_iterator_next(iterator: GhosttyRenderStateRowIterator) -> bool;
    fn ghostty_render_state_row_get(
        iterator: GhosttyRenderStateRowIterator,
        data: GhosttyRenderStateRowData,
        out: *mut c_void,
    ) -> GhosttyResult;
    fn ghostty_render_state_row_set(
        iterator: GhosttyRenderStateRowIterator,
        option: GhosttyRenderStateRowOption,
        value: *const c_void,
    ) -> GhosttyResult;
    fn ghostty_render_state_row_cells_new(
        allocator: *const c_void,
        out_cells: *mut GhosttyRenderStateRowCells,
    ) -> GhosttyResult;
    fn ghostty_render_state_row_cells_free(cells: GhosttyRenderStateRowCells);
    fn ghostty_render_state_row_cells_next(cells: GhosttyRenderStateRowCells) -> bool;
    fn ghostty_render_state_row_cells_get(
        cells: GhosttyRenderStateRowCells,
        data: GhosttyRenderStateRowCellsData,
        out: *mut c_void,
    ) -> GhosttyResult;
    fn ghostty_cell_get(
        cell: GhosttyCell,
        data: GhosttyCellData,
        out: *mut c_void,
    ) -> GhosttyResult;

    fn ghostty_key_encoder_new(
        allocator: *const c_void,
        encoder: *mut GhosttyKeyEncoder,
    ) -> GhosttyResult;
    fn ghostty_key_encoder_free(encoder: GhosttyKeyEncoder);
    fn ghostty_key_encoder_setopt_from_terminal(
        encoder: GhosttyKeyEncoder,
        terminal: GhosttyTerminal,
    );
    fn ghostty_key_encoder_encode(
        encoder: GhosttyKeyEncoder,
        event: GhosttyKeyEvent,
        out_buf: *mut c_char,
        out_buf_size: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;

    fn ghostty_key_event_new(
        allocator: *const c_void,
        event: *mut GhosttyKeyEvent,
    ) -> GhosttyResult;
    fn ghostty_key_event_free(event: GhosttyKeyEvent);
    fn ghostty_key_event_set_action(event: GhosttyKeyEvent, action: Action);
    fn ghostty_key_event_set_key(event: GhosttyKeyEvent, key: Key);
    fn ghostty_key_event_set_mods(event: GhosttyKeyEvent, mods: u16);
    fn ghostty_key_event_set_consumed_mods(event: GhosttyKeyEvent, mods: u16);
    fn ghostty_key_event_set_composing(event: GhosttyKeyEvent, composing: bool);
    fn ghostty_key_event_set_utf8(event: GhosttyKeyEvent, utf8: *const c_char, len: usize);
    fn ghostty_key_event_set_unshifted_codepoint(event: GhosttyKeyEvent, codepoint: u32);
}
