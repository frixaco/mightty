//! Terminal Widget
//!
//! GPUI component that renders terminal content and handles user interaction.
//! Combines ConPtyShell, libghostty Terminal, and key encoding into a complete
//! terminal widget.

use gpui::{
    div, prelude::*, px, Bounds, Context, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, Pixels, Render, Styled, Window,
};
use libghostty_vt::{
    key::{Action, Encoder, Event, Key, Mods},
    render::{CellIterator, RowIterator},
    style::{RgbColor, Underline},
    RenderState, Terminal, TerminalOptions,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{channel, Receiver, Sender},
    Arc, Condvar, Mutex,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(windows)]
use crate::shell::ConPtyShell;

/// Cursor style options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorStyle {
    #[default]
    Block,
    Line,
    Underline,
}

/// Terminal widget configuration
#[derive(Debug, Clone)]
pub struct TerminalConfig {
    pub shell: String,
    pub initial_rows: u16,
    pub initial_cols: u16,
    pub scrollback: usize,
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
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

type OutputData = Vec<u8>;
type IoWakeSignal = Arc<(Mutex<bool>, Condvar)>;

pub struct TerminalWidget {
    terminal: Terminal<'static, 'static>,
    key_encoder: Encoder<'static>,
    key_event: Event<'static>,
    render_state: RenderState<'static>,
    row_iterator: RowIterator<'static>,
    cell_iterator: CellIterator<'static>,
    config: TerminalConfig,
    output_rx: Receiver<OutputData>,
    input_tx: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    resize_tx: Option<Sender<(u16, u16)>>,
    io_wake: IoWakeSignal,
    shutdown_flag: Arc<AtomicBool>,
    exit_flag: Arc<AtomicBool>,
    io_thread: Option<JoinHandle<()>>,
    focus_handle: FocusHandle,
    cursor_blink_phase: bool,
    blink_accumulator: Duration,
    last_frame_time: Option<Instant>,
    size: (u16, u16),
    cell_size: (Pixels, Pixels),
    theme: TerminalTheme,
    has_exited: bool,
}

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
                gpui::rgba(0x000000),
                gpui::rgba(0xcd0000),
                gpui::rgba(0x00cd00),
                gpui::rgba(0xcdcd00),
                gpui::rgba(0x0000ee),
                gpui::rgba(0xcd00cd),
                gpui::rgba(0x00cdcd),
                gpui::rgba(0xe5e5e5),
                gpui::rgba(0x7f7f7f),
                gpui::rgba(0xff0000),
                gpui::rgba(0x00ff00),
                gpui::rgba(0xffff00),
                gpui::rgba(0x5c5cff),
                gpui::rgba(0xff00ff),
                gpui::rgba(0x00ffff),
                gpui::rgba(0xffffff),
            ],
        }
    }
}

fn rgb_to_rgba(rgb: RgbColor) -> gpui::Rgba {
    gpui::rgba((rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32)
}

fn cell_position(row: u16, col: u16, cell_size: (Pixels, Pixels)) -> (Pixels, Pixels) {
    (cell_size.0 * col as f32, cell_size.1 * row as f32)
}

impl TerminalWidget {
    pub fn new(config: TerminalConfig, cx: &mut Context<Self>) -> Self {
        let terminal = Terminal::new(TerminalOptions {
            cols: config.initial_cols,
            rows: config.initial_rows,
            max_scrollback: config.scrollback,
        })
        .expect("Failed to create terminal");

        let render_state = RenderState::new().expect("Failed to create render state");
        let row_iterator = RowIterator::new().expect("Failed to create row iterator");
        let cell_iterator = CellIterator::new().expect("Failed to create cell iterator");
        let key_encoder = Encoder::new().expect("Failed to create key encoder");
        let key_event = Event::new().expect("Failed to create key event");

        let (output_tx, output_rx) = channel::<OutputData>();
        let (input_tx, input_rx) = channel::<Vec<u8>>();
        let (resize_tx, resize_rx) = channel::<(u16, u16)>();
        let io_wake = Arc::new((Mutex::new(false), Condvar::new()));
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let exit_flag = Arc::new(AtomicBool::new(false));
        let exit_flag_thread = Arc::clone(&exit_flag);

        let shell_cmd = config.shell.clone();
        let rows = config.initial_rows;
        let cols = config.initial_cols;
        let shutdown_thread = Arc::clone(&shutdown_flag);
        let _io_wake_thread = Arc::clone(&io_wake);

        #[cfg(windows)]
        let io_thread = Some(std::thread::spawn(move || {
            let mut shell = match ConPtyShell::spawn(&shell_cmd, rows, cols) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Failed to spawn shell: {}", e);
                    exit_flag_thread.store(true, Ordering::Relaxed);
                    return;
                }
            };

            let mut buf = [0u8; 32768];
            let mut output_buffer: Vec<u8> = Vec::with_capacity(65536);
            let output_batch_threshold = 16384;

            loop {
                if shutdown_thread.load(Ordering::Relaxed) {
                    if !output_buffer.is_empty() {
                        let _ = output_tx.send(std::mem::take(&mut output_buffer));
                    }
                    let _ = shell.shutdown();
                    return;
                }

                let mut did_work = false;

                loop {
                    match input_rx.try_recv() {
                        Ok(data) => {
                            did_work = true;
                            if shell.write(&data).is_err() {
                                exit_flag_thread.store(true, Ordering::Relaxed);
                                return;
                            }
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            exit_flag_thread.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                }

                let mut pending_resize = None;
                loop {
                    match resize_rx.try_recv() {
                        Ok(size) => pending_resize = Some(size),
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            exit_flag_thread.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                }

                if let Some((rows, cols)) = pending_resize {
                    did_work = true;
                    if shell.resize(rows, cols).is_err() {
                        exit_flag_thread.store(true, Ordering::Relaxed);
                        return;
                    }
                }

                loop {
                    match shell.peek() {
                        Ok(true) => match shell.read(&mut buf) {
                            Ok(n) if n > 0 => {
                                did_work = true;
                                output_buffer.extend_from_slice(&buf[0..n]);
                                if output_buffer.len() >= output_batch_threshold {
                                    if output_tx.send(std::mem::take(&mut output_buffer)).is_err() {
                                        exit_flag_thread.store(true, Ordering::Relaxed);
                                        return;
                                    }
                                    output_buffer = Vec::with_capacity(65536);
                                }
                            }
                            Ok(_) => break,
                            Err(_) => {
                                exit_flag_thread.store(true, Ordering::Relaxed);
                                return;
                            }
                        },
                        Ok(false) => break,
                        Err(_) => {
                            exit_flag_thread.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                }

                if !output_buffer.is_empty() {
                    did_work = true;
                    if output_tx.send(std::mem::take(&mut output_buffer)).is_err() {
                        exit_flag_thread.store(true, Ordering::Relaxed);
                        return;
                    }
                    output_buffer = Vec::with_capacity(65536);
                }

                if !did_work {
                    std::thread::sleep(Duration::from_millis(8));
                }
            }
        }));

        #[cfg(not(windows))]
        let io_thread: Option<JoinHandle<()>> = {
            exit_flag.store(true, Ordering::Relaxed);
            None
        };

        let size = (config.initial_cols, config.initial_rows);

        Self {
            terminal,
            key_encoder,
            key_event,
            render_state,
            row_iterator,
            cell_iterator,
            config,
            output_rx,
            input_tx: Some(input_tx),
            resize_tx: Some(resize_tx),
            io_wake,
            shutdown_flag,
            exit_flag,
            io_thread,
            focus_handle: cx.focus_handle(),
            cursor_blink_phase: true,
            blink_accumulator: Duration::ZERO,
            last_frame_time: None,
            size,
            cell_size: (px(9.6), px(19.2)),
            theme: TerminalTheme::default(),
            has_exited: false,
        }
    }

    pub fn set_exit_flag(&mut self, flag: Arc<AtomicBool>) {
        self.exit_flag = flag;
    }

    pub fn has_exited(&self) -> bool {
        self.has_exited
    }

    pub fn check_exit(&mut self) -> bool {
        if !self.has_exited && self.exit_flag.load(Ordering::Relaxed) {
            self.has_exited = true;
            true
        } else {
            false
        }
    }

    pub fn request_focus(&self, window: &mut Window) {
        self.focus_handle.focus(window);
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

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

    fn wake_io_thread(&self) {
        let (lock, cvar) = &*self.io_wake;
        if let Ok(mut pending) = lock.lock() {
            *pending = true;
            cvar.notify_one();
        }
    }

    fn process_output(&mut self, cx: &mut Context<Self>) {
        let mut has_new_data = false;
        while let Ok(data) = self.output_rx.try_recv() {
            self.terminal.vt_write(&data);
            has_new_data = true;
        }
        if has_new_data {
            cx.notify();
        }
    }

    fn calculate_dimensions(&self, bounds: &Bounds<Pixels>) -> (u16, u16) {
        let cols = (bounds.size.width / self.cell_size.0).floor() as u16;
        let rows = (bounds.size.height / self.cell_size.1).floor() as u16;
        (cols.max(1), rows.max(1))
    }

    fn resize_to_bounds(&mut self, bounds: &Bounds<Pixels>, cx: &mut Context<Self>) {
        let (cols, rows) = self.calculate_dimensions(bounds);
        if cols != self.size.0 || rows != self.size.1 {
            let cell_width: f32 = self.cell_size.0.into();
            let cell_height: f32 = self.cell_size.1.into();
            if self
                .terminal
                .resize(cols, rows, cell_width as u32, cell_height as u32)
                .is_ok()
            {
                self.size = (cols, rows);
                if let Some(ref resize_tx) = self.resize_tx {
                    let _ = resize_tx.send((rows, cols));
                    self.wake_io_thread();
                }
                cx.notify();
            }
        }
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = if event.is_held {
            Action::Repeat
        } else {
            Action::Press
        };
        self.send_encoded_key(action, &event.keystroke, cx);
    }

    fn handle_key_up(&mut self, event: &KeyUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.send_encoded_key(Action::Release, &event.keystroke, cx);
    }

    fn send_encoded_key(
        &mut self,
        action: Action,
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

    fn encode_key_event(&mut self, action: Action, keystroke: &gpui::Keystroke) -> Option<Vec<u8>> {
        let ghostty_key = self.convert_to_ghostty_key(keystroke);
        let ghostty_mods = self.convert_to_ghostty_mods(keystroke);
        let printable_text = self.printable_text(keystroke, action);
        let unshifted_codepoint = self.unshifted_codepoint(keystroke);
        let consumed_mods = self.consumed_mods(
            &keystroke.key,
            ghostty_mods,
            printable_text.as_deref(),
            unshifted_codepoint,
        );

        self.key_event
            .set_action(action)
            .set_key(ghostty_key)
            .set_mods(ghostty_mods)
            .set_consumed_mods(consumed_mods)
            .set_unshifted_codepoint(unshifted_codepoint)
            .set_utf8(printable_text.as_deref());

        self.key_encoder.set_options_from_terminal(&self.terminal);

        let mut response = Vec::with_capacity(64);
        self.key_encoder
            .encode_to_vec(&self.key_event, &mut response)
            .ok()?;
        if response.is_empty() {
            None
        } else {
            Some(response)
        }
    }

    fn printable_text<'a>(
        &self,
        keystroke: &'a gpui::Keystroke,
        action: Action,
    ) -> Option<&'a str> {
        if action == Action::Release {
            return None;
        }
        keystroke
            .key_char
            .as_deref()
            .filter(|t| !t.is_empty())
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

    fn unshifted_codepoint(&self, keystroke: &gpui::Keystroke) -> char {
        if keystroke.key == "space" {
            return ' ';
        }
        let mut chars = keystroke.key.chars();
        let Some(c) = chars.next() else { return '\0' };
        if chars.next().is_some() {
            return '\0';
        }
        match c {
            'A'..='Z' => c.to_ascii_lowercase(),
            '!' => '1',
            '@' => '2',
            '#' => '3',
            '$' => '4',
            '%' => '5',
            '^' => '6',
            '&' => '7',
            '*' => '8',
            '(' => '9',
            ')' => '0',
            '_' => '-',
            '+' => '=',
            '{' => '[',
            '}' => ']',
            '|' => '\\',
            ':' => ';',
            '"' => '\'',
            '<' => ',',
            '>' => '.',
            '?' => '/',
            '~' => '`',
            _ => c,
        }
    }

    fn consumed_mods(&self, key: &str, mods: Mods, text: Option<&str>, ucp: char) -> Mods {
        let Some(t) = text else { return Mods::empty() };
        let mut chars = t.chars();
        let Some(tc) = chars.next() else {
            return Mods::empty();
        };
        if chars.next().is_some() {
            return Mods::empty();
        }
        if mods.contains(Mods::SHIFT) && tc != ucp {
            Mods::SHIFT
        } else if self.key_implies_shift(key, ucp) {
            Mods::SHIFT
        } else {
            Mods::empty()
        }
    }

    fn key_implies_shift(&self, key: &str, ucp: char) -> bool {
        let mut chars = key.chars();
        let Some(kc) = chars.next() else { return false };
        if chars.next().is_some() {
            return false;
        }
        ucp != '\0' && kc != ucp
    }

    fn convert_to_ghostty_key(&self, keystroke: &gpui::Keystroke) -> Key {
        match keystroke.key.as_str() {
            "up" => Key::ArrowUp,
            "down" => Key::ArrowDown,
            "left" => Key::ArrowLeft,
            "right" => Key::ArrowRight,
            "home" => Key::Home,
            "end" => Key::End,
            "insert" => Key::Insert,
            "delete" => Key::Delete,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "escape" => Key::Escape,
            "enter" => Key::Enter,
            "backspace" => Key::Backspace,
            "tab" => Key::Tab,
            "space" => Key::Space,
            "f1" => Key::F1,
            "f2" => Key::F2,
            "f3" => Key::F3,
            "f4" => Key::F4,
            "f5" => Key::F5,
            "f6" => Key::F6,
            "f7" => Key::F7,
            "f8" => Key::F8,
            "f9" => Key::F9,
            "f10" => Key::F10,
            "f11" => Key::F11,
            "f12" => Key::F12,
            _ if keystroke.key.len() == 1 => {
                let c = keystroke.key.chars().next().unwrap_or('?');
                match c.to_ascii_lowercase() {
                    'a'..='z' => match c {
                        'a' => Key::A,
                        'b' => Key::B,
                        'c' => Key::C,
                        'd' => Key::D,
                        'e' => Key::E,
                        'f' => Key::F,
                        'g' => Key::G,
                        'h' => Key::H,
                        'i' => Key::I,
                        'j' => Key::J,
                        'k' => Key::K,
                        'l' => Key::L,
                        'm' => Key::M,
                        'n' => Key::N,
                        'o' => Key::O,
                        'p' => Key::P,
                        'q' => Key::Q,
                        'r' => Key::R,
                        's' => Key::S,
                        't' => Key::T,
                        'u' => Key::U,
                        'v' => Key::V,
                        'w' => Key::W,
                        'x' => Key::X,
                        'y' => Key::Y,
                        'z' => Key::Z,
                        _ => Key::Unidentified,
                    },
                    '0'..='9' => Key::Digit0,
                    '-' => Key::Minus,
                    '=' => Key::Equal,
                    '[' => Key::BracketLeft,
                    ']' => Key::BracketRight,
                    ';' => Key::Semicolon,
                    '\'' => Key::Quote,
                    ',' => Key::Comma,
                    '.' => Key::Period,
                    '/' => Key::Slash,
                    '\\' => Key::Backslash,
                    '`' => Key::Backquote,
                    _ => Key::Unidentified,
                }
            }
            _ => Key::Unidentified,
        }
    }

    fn convert_to_ghostty_mods(&self, keystroke: &gpui::Keystroke) -> Mods {
        let mut mods = Mods::empty();
        if keystroke.modifiers.shift {
            mods |= Mods::SHIFT;
        }
        if keystroke.modifiers.alt {
            mods |= Mods::ALT;
        }
        if keystroke.modifiers.control {
            mods |= Mods::CTRL;
        }
        if keystroke.modifiers.platform {
            mods |= Mods::SUPER;
        }
        mods
    }

    fn handle_mouse_down(
        &mut self,
        _event: &MouseDownEvent,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
    }
}

impl Render for TerminalWidget {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();

        let now = Instant::now();
        if let Some(last_time) = self.last_frame_time {
            let elapsed = now.duration_since(last_time);
            let old_blink_phase = self.cursor_blink_phase;
            self.update_cursor_blink(elapsed);
            if old_blink_phase != self.cursor_blink_phase {
                cx.notify();
            }
        }
        self.last_frame_time = Some(now);

        self.process_output(cx);

        let bounds = window.bounds();
        self.resize_to_bounds(&bounds, cx);

        let snapshot = match self.render_state.update(&self.terminal) {
            Ok(s) => s,
            Err(_) => {
                return div()
                    .size_full()
                    .bg(self.theme.background)
                    .child("Failed to update render state")
            }
        };

        let colors = match snapshot.colors() {
            Ok(c) => c,
            Err(_) => return div().size_full().bg(self.theme.background),
        };

        let cell_size = self.cell_size;
        let mut elements: Vec<gpui::AnyElement> = Vec::new();

        let mut row_it = match self.row_iterator.update(&snapshot) {
            Ok(it) => it,
            Err(_) => return div().size_full().bg(self.theme.background),
        };

        let mut row_idx: u16 = 0;
        while let Some(row) = row_it.next() {
            let mut cell_it = match self.cell_iterator.update(row) {
                Ok(it) => it,
                Err(_) => continue,
            };

            let mut col_idx: u16 = 0;
            while let Some(cell) = cell_it.next() {
                let graphemes_len = match cell.graphemes_len() {
                    Ok(n) => n,
                    Err(_) => continue,
                };

                if graphemes_len == 0 {
                    col_idx += 1;
                    continue;
                }

                let text: String = match cell.graphemes() {
                    Ok(g) => g.into_iter().collect(),
                    Err(_) => continue,
                };

                let fg = cell.fg_color().ok().flatten().unwrap_or(colors.foreground);
                let bg = cell.bg_color().ok().flatten();
                let style = match cell.style() {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                let (fg_color, bg_color, has_bg) = if style.inverse {
                    (fg, bg.unwrap_or(colors.background), true)
                } else {
                    (fg, bg.unwrap_or(colors.background), bg.is_some())
                };

                let (x, y) = cell_position(row_idx, col_idx, cell_size);
                let fg_rgba = rgb_to_rgba(fg_color);
                let font_family = "JetBrainsMono Nerd Font";
                let font_weight = if style.bold {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                };

                let cell_div = div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(cell_size.0)
                    .h(cell_size.1)
                    .when(has_bg || style.inverse, |this| {
                        this.bg(rgb_to_rgba(bg_color))
                    })
                    .text_size(px(16.0))
                    .font_family(font_family)
                    .font_weight(font_weight)
                    .text_color(fg_rgba)
                    .when(style.italic, |this| this.italic())
                    .when(style.underline != Underline::None, |this| this.underline())
                    .when(style.strikethrough, |this| this.line_through())
                    .child(text);

                elements.push(cell_div.into_any_element());
                col_idx += 1;
            }
            let _ = row.set_dirty(false);
            row_idx += 1;
        }

        let is_focused = self.focus_handle.is_focused(window);
        let cursor_visible = is_focused && (self.cursor_blink_phase || !self.config.cursor_blink);

        if cursor_visible {
            if let Ok(Some(cursor_pos)) = snapshot.cursor_viewport() {
                let cursor_color = colors.cursor.unwrap_or(colors.foreground);
                let (x, y) = cell_position(cursor_pos.y, cursor_pos.x, cell_size);
                let cursor_rgba = rgb_to_rgba(cursor_color);

                let cursor_div = match self.config.cursor_style {
                    CursorStyle::Block => div()
                        .absolute()
                        .left(x)
                        .top(y)
                        .w(cell_size.0)
                        .h(cell_size.1)
                        .bg(cursor_rgba),
                    CursorStyle::Line => {
                        let (cw, ch) = (px(2.0), px(16.0));
                        let baseline = px(13.5);
                        div()
                            .absolute()
                            .left(x + px(1.0))
                            .top(y + baseline - ch / 2.0)
                            .w(cw)
                            .h(ch)
                            .bg(cursor_rgba)
                    }
                    CursorStyle::Underline => div()
                        .absolute()
                        .left(x)
                        .top(y + cell_size.1 - px(2.0))
                        .w(cell_size.0)
                        .h(px(2.0))
                        .bg(cursor_rgba),
                };
                elements.push(cursor_div.into_any_element());
            }
        }

        div()
            .size_full()
            .bg(rgb_to_rgba(colors.background))
            .relative()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key_down(event, window, cx)
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, window, cx| {
                this.handle_key_up(event, window, cx)
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.handle_mouse_down(event, window, cx)
                }),
            )
            .children(elements)
    }
}

impl Drop for TerminalWidget {
    fn drop(&mut self) {
        self.shutdown_flag.store(true, Ordering::Relaxed);
        self.wake_io_thread();
        self.input_tx = None;
        self.resize_tx = None;
        if let Some(handle) = self.io_thread.take() {
            let _ = handle.join();
        }
    }
}
