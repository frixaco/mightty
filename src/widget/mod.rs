//! Terminal Widget
//!
//! GPUI component that renders terminal content and handles user interaction.
//! Combines ConPtyShell, libghostty Terminal, and GhosttyKeyEncoder into a complete
//! terminal widget.

use gpui::{
    div, prelude::*, px, Bounds, Context, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, Pixels, Render, Styled, Window,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{channel, Receiver, Sender},
    Arc, Condvar, Mutex,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::ghostty::{
    ghostty_key_encoder_encode, ghostty_key_encoder_new, ghostty_key_encoder_setopt_from_terminal,
    ghostty_key_event_new, ghostty_key_event_set_action, ghostty_key_event_set_composing,
    ghostty_key_event_set_consumed_mods, ghostty_key_event_set_key, ghostty_key_event_set_mods,
    ghostty_key_event_set_unshifted_codepoint, ghostty_key_event_set_utf8, Cell, Color, GhosttyKey,
    GhosttyKeyAction, GhosttyKeyEncoder, GhosttyKeyEvent, GhosttyMods, GhosttyResult,
    GhosttyTerminalOptions, Terminal,
};
#[cfg(windows)]
use crate::shell::ConPtyShell;

/// Cursor style options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorStyle {
    /// Solid block cursor (default)
    #[default]
    Block,
    /// Vertical line cursor
    Line,
    /// Underline cursor
    Underline,
}

/// Terminal widget configuration
#[derive(Debug, Clone)]
pub struct TerminalConfig {
    /// Shell command to spawn (e.g., "cmd.exe", "pwsh.exe")
    pub shell: String,
    /// Initial terminal dimensions in cells
    pub initial_rows: u16,
    pub initial_cols: u16,
    /// Scrollback buffer size
    pub scrollback: usize,
    /// Cursor style
    pub cursor_style: CursorStyle,
    /// Enable cursor blinking
    pub cursor_blink: bool,
    /// Cursor blink interval
    pub blink_interval: Duration,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: "pwsh.exe".to_string(),
            initial_rows: 24,
            initial_cols: 80,
            scrollback: 1000,
            cursor_style: CursorStyle::Line,
            cursor_blink: true,
            blink_interval: Duration::from_millis(500),
        }
    }
}

/// Output message from shell reader thread
type OutputData = Vec<u8>;

/// Wake signal for the shell I/O thread
type IoWakeSignal = Arc<(Mutex<bool>, Condvar)>;

/// Terminal widget state and rendering
pub struct TerminalWidget {
    /// libghostty terminal emulator
    terminal: Terminal,
    /// Key encoder for VT sequence generation
    key_encoder: GhosttyKeyEncoder,
    /// Key event for encoding
    key_event: GhosttyKeyEvent,
    /// Configuration
    config: TerminalConfig,
    /// Output data receiver from reader thread
    output_rx: Receiver<OutputData>,
    /// Input data sender to shell (for sending keypresses to shell)
    input_tx: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    /// Resize event sender to shell I/O thread
    resize_tx: Option<Sender<(u16, u16)>>,
    /// Wake signal for the shell I/O thread
    io_wake: IoWakeSignal,
    /// Shutdown flag for the I/O thread
    shutdown_flag: Arc<AtomicBool>,
    /// Handle to the I/O thread
    io_thread: Option<JoinHandle<()>>,
    /// Focus handle for tracking focus
    focus_handle: FocusHandle,
    /// Cursor blink state (visible/hidden)
    cursor_blink_phase: bool,
    /// Accumulated time for cursor blink
    blink_accumulator: Duration,
    /// Time of last frame for blink calculation
    last_frame_time: Option<Instant>,
    /// Current cursor position (col, row)
    cursor_pos: (u16, u16),
    /// Terminal dimensions in cells
    size: (u16, u16),
    /// Cell dimensions in pixels (width, height)
    cell_size: (Pixels, Pixels),
    /// Theme colors
    theme: TerminalTheme,
}

/// Terminal color theme
#[derive(Debug, Clone)]
pub struct TerminalTheme {
    pub foreground: gpui::Rgba,
    pub background: gpui::Rgba,
    pub cursor: gpui::Rgba,
    pub selection: gpui::Rgba,
    pub palette: [gpui::Rgba; 16],
}

impl Default for TerminalTheme {
    fn default() -> Self {
        Self {
            foreground: gpui::rgba(0xc0c0c0),
            background: gpui::rgba(0x1a1a1a),
            cursor: gpui::rgba(0xffffff),
            selection: gpui::rgba(0x3d3d3d),
            palette: [
                gpui::rgba(0x000000), // Black
                gpui::rgba(0xcd0000), // Red
                gpui::rgba(0x00cd00), // Green
                gpui::rgba(0xcdcd00), // Yellow
                gpui::rgba(0x0000ee), // Blue
                gpui::rgba(0xcd00cd), // Magenta
                gpui::rgba(0x00cdcd), // Cyan
                gpui::rgba(0xe5e5e5), // White
                gpui::rgba(0x7f7f7f), // Bright Black
                gpui::rgba(0xff0000), // Bright Red
                gpui::rgba(0x00ff00), // Bright Green
                gpui::rgba(0xffff00), // Bright Yellow
                gpui::rgba(0x5c5cff), // Bright Blue
                gpui::rgba(0xff00ff), // Bright Magenta
                gpui::rgba(0x00ffff), // Bright Cyan
                gpui::rgba(0xffffff), // Bright White
            ],
        }
    }
}

impl TerminalWidget {
    /// Create a new terminal widget with the given configuration
    pub fn new(config: TerminalConfig, cx: &mut Context<Self>) -> Self {
        // Initialize libghostty terminal
        let terminal = Terminal::new(GhosttyTerminalOptions {
            cols: config.initial_cols,
            rows: config.initial_rows,
            max_scrollback: config.scrollback,
        })
        .expect("Failed to create terminal");

        // Spawn shell and create channels
        let (output_tx, output_rx) = channel::<OutputData>();
        let (input_tx, input_rx) = channel::<Vec<u8>>();
        let (resize_tx, resize_rx) = channel::<(u16, u16)>();
        let io_wake: IoWakeSignal = Arc::new((Mutex::new(false), Condvar::new()));
        let shutdown_flag = Arc::new(AtomicBool::new(false));

        // Clone config for the thread
        let shell_cmd = config.shell.clone();
        let rows = config.initial_rows;
        let cols = config.initial_cols;
        let shutdown_thread = Arc::clone(&shutdown_flag);

        // I/O thread wake signal (kept for potential future use with event-based I/O)
        let _io_wake_thread = Arc::clone(&io_wake);

        // Start I/O thread that handles both reading and writing (Windows only)
        #[cfg(windows)]
        let io_thread = Some(std::thread::spawn(move || {
            let mut shell = match ConPtyShell::spawn(&shell_cmd, rows, cols) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Failed to spawn shell: {}", e);
                    return;
                }
            };

            let mut buf = [0u8; 32768]; // 32KB buffer for efficient burst reading
            let mut output_buffer: Vec<u8> = Vec::with_capacity(65536); // 64KB output buffer
            let output_batch_threshold = 16384; // Send when buffer reaches 16KB

            loop {
                // Check shutdown flag
                if shutdown_thread.load(Ordering::Relaxed) {
                    // Flush remaining output before shutdown
                    if !output_buffer.is_empty() {
                        let _ = output_tx.send(std::mem::take(&mut output_buffer));
                    }
                    let _ = shell.shutdown();
                    return;
                }

                let mut did_work = false;

                // Drain all pending input immediately so interactive input doesn't wait
                // for the next polling tick.
                loop {
                    match input_rx.try_recv() {
                        Ok(data) => {
                            did_work = true;
                            if shell.write(&data).is_err() {
                                return;
                            }
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    }
                }

                // Keep only the latest resize and apply it before reading output.
                let mut pending_resize = None;
                loop {
                    match resize_rx.try_recv() {
                        Ok(size) => {
                            pending_resize = Some(size);
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    }
                }

                if let Some((rows, cols)) = pending_resize {
                    did_work = true;
                    if shell.resize(rows, cols).is_err() {
                        return;
                    }
                }

                // Drain all available output into buffer
                loop {
                    match shell.peek() {
                        Ok(true) => match shell.read(&mut buf) {
                            Ok(n) if n > 0 => {
                                did_work = true;
                                output_buffer.extend_from_slice(&buf[0..n]);
                                // Send batch if buffer is large enough
                                if output_buffer.len() >= output_batch_threshold {
                                    if output_tx.send(std::mem::take(&mut output_buffer)).is_err() {
                                        return;
                                    }
                                    output_buffer = Vec::with_capacity(65536);
                                }
                            }
                            Ok(_) => break,
                            Err(_) => return,
                        },
                        Ok(false) => break,
                        Err(_) => return,
                    }
                }

                // Flush output buffer if we have data
                if !output_buffer.is_empty() {
                    did_work = true;
                    if output_tx.send(std::mem::take(&mut output_buffer)).is_err() {
                        return;
                    }
                    output_buffer = Vec::with_capacity(65536);
                }

                if !did_work {
                    // Use brief sleep instead of polling - reduces CPU usage
                    std::thread::sleep(Duration::from_millis(8));
                }
            }
        }));

        #[cfg(not(windows))]
        let io_thread: Option<JoinHandle<()>> = None;

        let size = (config.initial_cols, config.initial_rows);

        // Create key encoder and event
        let mut key_encoder: GhosttyKeyEncoder = std::ptr::null_mut();
        let mut key_event: GhosttyKeyEvent = std::ptr::null_mut();

        unsafe {
            if GhosttyResult::Success == ghostty_key_encoder_new(std::ptr::null(), &mut key_encoder)
            {
                ghostty_key_encoder_setopt_from_terminal(key_encoder, terminal.as_ptr());
            }
            ghostty_key_event_new(std::ptr::null(), &mut key_event);
        }

        let this = Self {
            terminal,
            key_encoder,
            key_event,
            config,
            output_rx,
            input_tx: Some(input_tx),
            resize_tx: Some(resize_tx),
            io_wake,
            shutdown_flag,
            io_thread,
            focus_handle: cx.focus_handle(),
            cursor_blink_phase: true,
            blink_accumulator: Duration::ZERO,
            last_frame_time: None,
            cursor_pos: (0, 0),
            size,
            cell_size: (px(8.4), px(16.8)), // Monospace cell size (width, height) - 14px font with 0.6 width and 1.2 line height
            theme: TerminalTheme::default(),
        };

        this
    }

    /// Update cursor blink state based on elapsed time
    fn update_cursor_blink(&mut self, elapsed: Duration) {
        if !self.config.cursor_blink {
            self.cursor_blink_phase = true;
            self.blink_accumulator = Duration::ZERO;
            return;
        }

        self.blink_accumulator += elapsed;
        if self.blink_accumulator >= self.config.blink_interval {
            self.blink_accumulator = Duration::ZERO;
            self.cursor_blink_phase = !self.cursor_blink_phase;
        }
    }

    /// Create a terminal widget with default configuration
    pub fn default(cx: &mut Context<Self>) -> Self {
        Self::new(TerminalConfig::default(), cx)
    }

    fn wake_io_thread(&self) {
        let (lock, cvar) = &*self.io_wake;
        if let Ok(mut pending) = lock.lock() {
            *pending = true;
            cvar.notify_one();
        }
    }

    /// Process any pending output from the shell
    fn process_output(&mut self, cx: &mut Context<Self>) {
        let mut has_new_data = false;

        // Drain all available output
        while let Ok(data) = self.output_rx.try_recv() {
            self.terminal.write(&data);
            has_new_data = true;
        }

        // Update cursor position from terminal
        self.cursor_pos = self.terminal.cursor_pos();

        // Notify GPUI to re-render if we got new data
        if has_new_data {
            cx.notify();
        }
    }

    /// Calculate terminal dimensions from bounds
    fn calculate_dimensions(&self, bounds: &Bounds<Pixels>) -> (u16, u16) {
        let cols = (bounds.size.width / self.cell_size.0).floor() as u16;
        let rows = (bounds.size.height / self.cell_size.1).floor() as u16;
        (cols.max(1), rows.max(1))
    }

    /// Resize the terminal to match the given bounds
    fn resize_to_bounds(&mut self, bounds: &Bounds<Pixels>, cx: &mut Context<Self>) {
        let (cols, rows) = self.calculate_dimensions(bounds);

        // Only resize if dimensions changed
        if cols != self.size.0 || rows != self.size.1 {
            // Resize ghostty terminal
            if self.terminal.resize(cols, rows).is_ok() {
                self.size = (cols, rows);

                // Also resize the shell
                if let Some(ref resize_tx) = self.resize_tx {
                    let _ = resize_tx.send((rows, cols));
                    self.wake_io_thread();
                }

                cx.notify();
            }
        }
    }

    /// Handle key down events
    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = if event.is_held {
            GhosttyKeyAction::Repeat
        } else {
            GhosttyKeyAction::Press
        };

        self.send_encoded_key(action, &event.keystroke, cx);
    }

    fn handle_key_up(&mut self, event: &KeyUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.send_encoded_key(GhosttyKeyAction::Release, &event.keystroke, cx);
    }

    fn send_encoded_key(
        &mut self,
        action: GhosttyKeyAction,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) {
        let input_tx = match &self.input_tx {
            Some(tx) => tx.clone(),
            None => return,
        };

        if let Some(vt_bytes) = self.encode_key_event(action, keystroke) {
            if let Err(e) = input_tx.send(vt_bytes) {
                eprintln!("Failed to send input to shell: {:?}", e);
                return;
            }

            self.wake_io_thread();
        }

        self.process_output(cx);
        cx.notify();
    }

    fn encode_key_event(
        &mut self,
        action: GhosttyKeyAction,
        keystroke: &gpui::Keystroke,
    ) -> Option<Vec<u8>> {
        unsafe {
            ghostty_key_encoder_setopt_from_terminal(self.key_encoder, self.terminal.as_ptr());
        }

        let ghostty_key = self.convert_to_ghostty_key(keystroke);
        let ghostty_mods = self.convert_to_ghostty_mods(keystroke);
        let printable_text = self.printable_text(keystroke, action);
        let unshifted_codepoint = self.unshifted_codepoint(keystroke);
        let consumed_mods = self.consumed_mods(
            &keystroke.key,
            ghostty_mods,
            printable_text,
            unshifted_codepoint,
        );

        unsafe {
            ghostty_key_event_set_action(self.key_event, action);
            ghostty_key_event_set_key(self.key_event, ghostty_key);
            ghostty_key_event_set_mods(self.key_event, ghostty_mods);
            ghostty_key_event_set_consumed_mods(self.key_event, consumed_mods);
            ghostty_key_event_set_unshifted_codepoint(self.key_event, unshifted_codepoint);
            ghostty_key_event_set_composing(self.key_event, false);

            if let Some(text) = printable_text {
                ghostty_key_event_set_utf8(self.key_event, text.as_ptr() as *const i8, text.len());
            } else {
                ghostty_key_event_set_utf8(self.key_event, std::ptr::null(), 0);
            }
        }

        let mut out_buf = [0i8; 256];
        let mut out_len: usize = 0;

        unsafe {
            let result = ghostty_key_encoder_encode(
                self.key_encoder,
                self.key_event,
                out_buf.as_mut_ptr(),
                out_buf.len(),
                &mut out_len,
            );

            if result == GhosttyResult::Success && out_len > 0 {
                Some(out_buf[0..out_len].iter().map(|&b| b as u8).collect())
            } else {
                None
            }
        }
    }

    fn printable_text<'a>(
        &self,
        keystroke: &'a gpui::Keystroke,
        action: GhosttyKeyAction,
    ) -> Option<&'a str> {
        if action == GhosttyKeyAction::Release {
            return None;
        }

        keystroke
            .key_char
            .as_deref()
            .filter(|text| !text.is_empty())
            .or_else(|| {
                if keystroke.key == "space" {
                    Some(" ")
                } else if keystroke.key.chars().count() == 1 {
                    Some(keystroke.key.as_str())
                } else {
                    None
                }
            })
    }

    fn unshifted_codepoint(&self, keystroke: &gpui::Keystroke) -> u32 {
        if keystroke.key == "space" {
            return ' ' as u32;
        }

        let mut chars = keystroke.key.chars();
        let Some(c) = chars.next() else {
            return 0;
        };

        if chars.next().is_some() {
            return 0;
        }

        match c {
            'A'..='Z' => c.to_ascii_lowercase() as u32,
            '!' => '1' as u32,
            '@' => '2' as u32,
            '#' => '3' as u32,
            '$' => '4' as u32,
            '%' => '5' as u32,
            '^' => '6' as u32,
            '&' => '7' as u32,
            '*' => '8' as u32,
            '(' => '9' as u32,
            ')' => '0' as u32,
            '_' => '-' as u32,
            '+' => '=' as u32,
            '{' => '[' as u32,
            '}' => ']' as u32,
            '|' => '\\' as u32,
            ':' => ';' as u32,
            '"' => '\'' as u32,
            '<' => ',' as u32,
            '>' => '.' as u32,
            '?' => '/' as u32,
            '~' => '`' as u32,
            _ => c as u32,
        }
    }

    fn consumed_mods(
        &self,
        key: &str,
        ghostty_mods: GhosttyMods,
        printable_text: Option<&str>,
        unshifted_codepoint: u32,
    ) -> GhosttyMods {
        let Some(text) = printable_text else {
            return GhosttyMods::default();
        };

        let mut chars = text.chars();
        let Some(text_char) = chars.next() else {
            return GhosttyMods::default();
        };

        if chars.next().is_some() {
            return GhosttyMods::default();
        }

        if ghostty_mods.contains(GhosttyMods::SHIFT) && text_char as u32 != unshifted_codepoint {
            GhosttyMods::SHIFT
        } else if self.key_implies_shift(key, unshifted_codepoint) {
            GhosttyMods::SHIFT
        } else {
            GhosttyMods::default()
        }
    }

    fn key_implies_shift(&self, key: &str, unshifted_codepoint: u32) -> bool {
        let mut chars = key.chars();
        let Some(key_char) = chars.next() else {
            return false;
        };

        if chars.next().is_some() {
            return false;
        }

        unshifted_codepoint > 0 && key_char as u32 != unshifted_codepoint
    }

    /// Convert GPUI keystroke to GhosttyKey
    fn convert_to_ghostty_key(&self, keystroke: &gpui::Keystroke) -> GhosttyKey {
        let key_str = keystroke.key.as_str();

        match key_str {
            // Arrows
            "up" => GhosttyKey::ArrowUp,
            "down" => GhosttyKey::ArrowDown,
            "left" => GhosttyKey::ArrowLeft,
            "right" => GhosttyKey::ArrowRight,
            // Control Pad
            "home" => GhosttyKey::Home,
            "end" => GhosttyKey::End,
            "insert" => GhosttyKey::Insert,
            "delete" => GhosttyKey::Delete,
            "pageup" => GhosttyKey::PageUp,
            "pagedown" => GhosttyKey::PageDown,
            // Function Keys
            "escape" => GhosttyKey::Escape,
            "enter" => GhosttyKey::Enter,
            "backspace" => GhosttyKey::Backspace,
            "tab" => GhosttyKey::Tab,
            "space" => GhosttyKey::Space,
            "f1" => GhosttyKey::F1,
            "f2" => GhosttyKey::F2,
            "f3" => GhosttyKey::F3,
            "f4" => GhosttyKey::F4,
            "f5" => GhosttyKey::F5,
            "f6" => GhosttyKey::F6,
            "f7" => GhosttyKey::F7,
            "f8" => GhosttyKey::F8,
            "f9" => GhosttyKey::F9,
            "f10" => GhosttyKey::F10,
            "f11" => GhosttyKey::F11,
            "f12" => GhosttyKey::F12,
            // Single character keys
            _ if keystroke.key.len() == 1 => {
                let c = keystroke.key.chars().next().unwrap_or('?');
                match c {
                    'a' => GhosttyKey::KeyA,
                    'b' => GhosttyKey::KeyB,
                    'c' => GhosttyKey::KeyC,
                    'd' => GhosttyKey::KeyD,
                    'e' => GhosttyKey::KeyE,
                    'f' => GhosttyKey::KeyF,
                    'g' => GhosttyKey::KeyG,
                    'h' => GhosttyKey::KeyH,
                    'i' => GhosttyKey::KeyI,
                    'j' => GhosttyKey::KeyJ,
                    'k' => GhosttyKey::KeyK,
                    'l' => GhosttyKey::KeyL,
                    'm' => GhosttyKey::KeyM,
                    'n' => GhosttyKey::KeyN,
                    'o' => GhosttyKey::KeyO,
                    'p' => GhosttyKey::KeyP,
                    'q' => GhosttyKey::KeyQ,
                    'r' => GhosttyKey::KeyR,
                    's' => GhosttyKey::KeyS,
                    't' => GhosttyKey::KeyT,
                    'u' => GhosttyKey::KeyU,
                    'v' => GhosttyKey::KeyV,
                    'w' => GhosttyKey::KeyW,
                    'x' => GhosttyKey::KeyX,
                    'y' => GhosttyKey::KeyY,
                    'z' => GhosttyKey::KeyZ,
                    '0' => GhosttyKey::Digit0,
                    '1' => GhosttyKey::Digit1,
                    '2' => GhosttyKey::Digit2,
                    '3' => GhosttyKey::Digit3,
                    '4' => GhosttyKey::Digit4,
                    '5' => GhosttyKey::Digit5,
                    '6' => GhosttyKey::Digit6,
                    '7' => GhosttyKey::Digit7,
                    '8' => GhosttyKey::Digit8,
                    '9' => GhosttyKey::Digit9,
                    ' ' => GhosttyKey::Space,
                    '-' => GhosttyKey::Minus,
                    '=' => GhosttyKey::Equal,
                    '[' => GhosttyKey::BracketLeft,
                    ']' => GhosttyKey::BracketRight,
                    ';' => GhosttyKey::Semicolon,
                    '\'' => GhosttyKey::Quote,
                    ',' => GhosttyKey::Comma,
                    '.' => GhosttyKey::Period,
                    '/' => GhosttyKey::Slash,
                    '\\' => GhosttyKey::Backslash,
                    '`' => GhosttyKey::Backquote,
                    _ => GhosttyKey::Unidentified,
                }
            }
            _ => GhosttyKey::Unidentified,
        }
    }

    /// Convert GPUI modifiers to GhosttyMods
    fn convert_to_ghostty_mods(&self, keystroke: &gpui::Keystroke) -> GhosttyMods {
        let mut mods = GhosttyMods::default();
        let keystroke_mods = &keystroke.modifiers;

        if keystroke_mods.shift {
            mods = mods | GhosttyMods::SHIFT;
        }
        if keystroke_mods.alt {
            mods = mods | GhosttyMods::ALT;
        }
        if keystroke_mods.control {
            mods = mods | GhosttyMods::CTRL;
        }
        if keystroke_mods.platform {
            mods = mods | GhosttyMods::SUPER;
        }

        mods
    }

    /// Handle mouse down events to focus
    fn handle_mouse_down(
        &mut self,
        _event: &MouseDownEvent,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
    }

    /// Convert Color to GPUI Rgba
    fn color_to_rgba(&self, color: &Color, is_background: bool) -> gpui::Rgba {
        match color {
            Color::Default => {
                if is_background {
                    self.theme.background
                } else {
                    self.theme.foreground
                }
            }
            Color::Palette(idx) => self.theme.palette[*idx as usize % 16],
            Color::Rgb(r, g, b) => {
                gpui::rgba(((*r as u32) << 16) | ((*g as u32) << 8) | (*b as u32))
            }
        }
    }

    /// Calculate cell position in pixels
    fn cell_position(&self, row: u16, col: u16) -> (Pixels, Pixels) {
        let x = self.cell_size.0 * col as f32;
        let y = self.cell_size.1 * row as f32;
        (x, y)
    }

    /// Render a single cell
    fn render_cell(&self, cell: &Cell, row: u16, col: u16) -> impl IntoElement {
        let (x, y) = self.cell_position(row, col);
        let bg = self.color_to_rgba(&cell.bg, true);
        let fg = self.color_to_rgba(&cell.fg, false);

        let font_family = if cell.attrs.bold {
            "JetBrains Mono"
        } else {
            "JetBrainsMono NF"
        };
        let font_weight = if cell.attrs.bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        };

        div()
            .absolute()
            .left(x)
            .top(y)
            .w(self.cell_size.0)
            .h(self.cell_size.1)
            .bg(bg)
            .text_size(px(14.0))
            .font_family(font_family)
            .font_weight(font_weight)
            .text_color(fg)
            .when(cell.attrs.italic, |this| this.italic())
            .when(cell.attrs.underline > 0, |this| this.underline())
            .when(cell.attrs.strikethrough, |this| this.line_through())
            .child(cell.char.to_string())
    }

    /// Build the cursor element
    fn build_cursor_element(&self, is_focused: bool) -> gpui::AnyElement {
        // Get cursor position from terminal
        let (cursor_col, cursor_row) = self.terminal.cursor_pos();
        let (x, y) = self.cell_position(cursor_row, cursor_col);

        // Determine cursor visibility - show if focused and (blinking phase or blink disabled)
        let cursor_visible = is_focused && (self.cursor_blink_phase || !self.config.cursor_blink);

        if !cursor_visible {
            return div().into_any_element();
        }

        let cursor_div = match self.config.cursor_style {
            CursorStyle::Block => div()
                .absolute()
                .left(x)
                .top(y)
                .w(self.cell_size.0)
                .h(self.cell_size.1)
                .bg(self.theme.cursor),
            CursorStyle::Line => {
                let cursor_width = px(2.0);
                let cursor_height = px(14.0);
                let baseline_in_cell = px(11.0);
                let cursor_top_in_cell = baseline_in_cell - (cursor_height / 2.0);
                div()
                    .absolute()
                    .left(x + px(1.0))
                    .top(y + cursor_top_in_cell)
                    .w(cursor_width)
                    .h(cursor_height)
                    .bg(self.theme.cursor)
            }
            CursorStyle::Underline => div()
                .absolute()
                .left(x)
                .top(y + self.cell_size.1 - px(2.0))
                .w(self.cell_size.0)
                .h(px(2.0))
                .bg(self.theme.cursor),
        };

        cursor_div.into_any_element()
    }
}

impl Render for TerminalWidget {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();

        // Update cursor blink timing
        let now = Instant::now();
        if let Some(last_time) = self.last_frame_time {
            let elapsed = now.duration_since(last_time);
            let old_blink_phase = self.cursor_blink_phase;
            self.update_cursor_blink(elapsed);
            // Only force re-render if blink state changed (for smooth cursor blink)
            if old_blink_phase != self.cursor_blink_phase {
                cx.notify(); // Trigger re-render for cursor blink
            }
        }
        self.last_frame_time = Some(now);

        // Process any new output from shell
        self.process_output(cx);

        // Calculate container bounds for resize
        let bounds = window.bounds();
        self.resize_to_bounds(&bounds, cx);

        // Read screen buffer and clone cells for rendering
        let (cursor_col, cursor_row) = self.terminal.cursor_pos();
        let buffer = self.terminal.read_screen();
        self.size = (buffer.cols, buffer.rows);

        // Exclude cursor cell from rendering if using block cursor
        let exclude_cursor = self.config.cursor_style == CursorStyle::Block;

        let cells_to_render: Vec<(u16, u16, Cell)> = buffer
            .cells
            .iter()
            .enumerate()
            .flat_map(|(row_idx, row_cells)| {
                row_cells
                    .iter()
                    .enumerate()
                    .filter_map(move |(col_idx, cell)| {
                        // Skip the cell under the cursor if using block cursor
                        if exclude_cursor
                            && row_idx as u16 == cursor_row
                            && col_idx as u16 == cursor_col
                        {
                            return None;
                        }

                        let has_content = cell.char != '\0' && cell.char != ' ';
                        let has_custom_bg = !matches!(cell.bg, Color::Default);
                        if has_content || has_custom_bg {
                            Some((row_idx as u16, col_idx as u16, *cell))
                        } else {
                            None
                        }
                    })
            })
            .collect();

        // Build the terminal content
        let mut elements: Vec<gpui::AnyElement> = Vec::new();
        for (row, col, cell) in cells_to_render {
            elements.push(self.render_cell(&cell, row, col).into_any_element());
        }

        // Build cursor element
        let is_focused = self.focus_handle.is_focused(window);
        let cursor_element = self.build_cursor_element(is_focused);
        elements.push(cursor_element);

        div()
            .size_full()
            .bg(self.theme.background)
            .relative()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key_down(event, window, cx);
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, window, cx| {
                this.handle_key_up(event, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.handle_mouse_down(event, window, cx);
                }),
            )
            .children(elements)
    }
}

impl Drop for TerminalWidget {
    fn drop(&mut self) {
        // Signal shutdown and wake the I/O thread
        self.shutdown_flag.store(true, Ordering::Relaxed);
        self.wake_io_thread();

        // Drop the senders to signal disconnection
        self.input_tx = None;
        self.resize_tx = None;

        // Join the I/O thread
        if let Some(handle) = self.io_thread.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = TerminalConfig::default();
        assert_eq!(config.shell, "pwsh.exe");
        assert_eq!(config.initial_rows, 24);
        assert_eq!(config.initial_cols, 80);
        assert!(config.cursor_blink);
    }

    #[test]
    fn test_theme_default() {
        let theme = TerminalTheme::default();
        assert_eq!(theme.palette.len(), 16);
    }

    #[test]
    fn test_cursor_style_default() {
        let style: CursorStyle = Default::default();
        assert_eq!(style, CursorStyle::Block);
    }
}
