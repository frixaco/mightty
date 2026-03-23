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

// ============================================================================
// Key Encoder Types
// ============================================================================

/// Opaque handle to a key encoder instance
pub type GhosttyKeyEncoder = *mut std::ffi::c_void;

/// Opaque handle to a key event
pub type GhosttyKeyEvent = *mut std::ffi::c_void;

/// Keyboard input event types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhosttyKeyAction {
    /// Key was released
    Release = 0,
    /// Key was pressed
    Press = 1,
    /// Key is being repeated (held down)
    Repeat = 2,
}

/// Keyboard modifier keys bitmask
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyMods(pub u16);

impl GhosttyMods {
    pub const SHIFT: Self = GhosttyMods(1 << 0);
    pub const CTRL: Self = GhosttyMods(1 << 1);
    pub const ALT: Self = GhosttyMods(1 << 2);
    pub const SUPER: Self = GhosttyMods(1 << 3);
    pub const CAPS_LOCK: Self = GhosttyMods(1 << 4);
    pub const NUM_LOCK: Self = GhosttyMods(1 << 5);

    pub fn contains(&self, other: GhosttyMods) -> bool {
        (self.0 & other.0) != 0
    }
}

impl std::ops::BitOr for GhosttyMods {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        GhosttyMods(self.0 | rhs.0)
    }
}

/// Key encoder option identifiers
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyKeyEncoderOption {
    CursorKeyApplication = 0,
    KeypadKeyApplication = 1,
    IgnoreKeypadWithNumlock = 2,
    AltEscPrefix = 3,
    ModifyOtherKeysState2 = 4,
    KittyFlags = 5,
    MacosOptionAsAlt = 6,
}

/// Physical key codes (W3C UI Events KeyboardEvent code standard)
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhosttyKey {
    Unidentified = 0,

    // Writing System Keys
    Backquote,
    Backslash,
    BracketLeft,
    BracketRight,
    Comma,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Equal,
    IntlBackslash,
    IntlRo,
    IntlYen,
    KeyA,
    KeyB,
    KeyC,
    KeyD,
    KeyE,
    KeyF,
    KeyG,
    KeyH,
    KeyI,
    KeyJ,
    KeyK,
    KeyL,
    KeyM,
    KeyN,
    KeyO,
    KeyP,
    KeyQ,
    KeyR,
    KeyS,
    KeyT,
    KeyU,
    KeyV,
    KeyW,
    KeyX,
    KeyY,
    KeyZ,
    Minus,
    Period,
    Quote,
    Semicolon,
    Slash,

    // Functional Keys
    AltLeft,
    AltRight,
    Backspace,
    CapsLock,
    ContextMenu,
    ControlLeft,
    ControlRight,
    Enter,
    MetaLeft,
    MetaRight,
    ShiftLeft,
    ShiftRight,
    Space,
    Tab,
    Convert,
    KanaMode,
    NonConvert,

    // Control Pad Section
    Delete,
    End,
    Help,
    Home,
    Insert,
    PageDown,
    PageUp,

    // Arrow Pad Section
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,

    // Numpad Section
    NumLock,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    NumpadBackspace,
    NumpadClear,
    NumpadClearEntry,
    NumpadComma,
    NumpadDecimal,
    NumpadDivide,
    NumpadEnter,
    NumpadEqual,
    NumpadMemoryAdd,
    NumpadMemoryClear,
    NumpadMemoryRecall,
    NumpadMemoryStore,
    NumpadMemorySubtract,
    NumpadMultiply,
    NumpadParenLeft,
    NumpadParenRight,
    NumpadSubtract,
    NumpadSeparator,
    NumpadUp,
    NumpadDown,
    NumpadRight,
    NumpadLeft,
    NumpadBegin,
    NumpadHome,
    NumpadEnd,
    NumpadInsert,
    NumpadPageUp,
    NumpadPageDown,

    // Function Section
    Escape,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
    F25,
    Fn,
    FnLock,
    PrintScreen,
    ScrollLock,
    Pause,

    // Media Keys
    BrowserBack,
    BrowserFavorites,
    BrowserForward,
    BrowserHome,
    BrowserRefresh,
    BrowserSearch,
    BrowserStop,
    Eject,
    LaunchApp1,
    LaunchApp2,
    LaunchMail,
    MediaPlayPause,
    MediaSelect,
    MediaStop,
    MediaTrackNext,
    MediaTrackPrevious,
    Power,
    Sleep,
    AudioVolumeDown,
    AudioVolumeMute,
    AudioVolumeUp,
    WakeUp,

    // Legacy, Non-standard, and Special Keys
    Copy,
    Cut,
    Paste,
}
