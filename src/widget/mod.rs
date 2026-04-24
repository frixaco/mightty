//! Terminal Widget
//!
//! GPUI component that renders terminal content and handles user interaction.
//! Combines ConPtyShell, libghostty Terminal, and key encoding into a complete
//! terminal widget.

use crate::feedback::{
    self, CaptureCell, CaptureColors, CaptureCursor, CaptureRow, FontCapture, GridSize, RgbHex,
    SizePx, TerminalCapture,
};
use crate::ghostty::{
    RenderState, Terminal, TerminalOptions,
    key::{Action, Encoder, Event, Key, Mods},
    render::{CellIterator, CellWidth, RowIterator},
    style::{RgbColor, Underline},
};
use gpui::{
    Bounds, Context, FocusHandle, FontFallbacks, FontFeatures, FontStyle, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, Pixels,
    Render, StrikethroughStyle, Styled, StyledText, TextRun, TextStyle, UnderlineStyle, WhiteSpace,
    Window, div, prelude::*, px,
};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, Sender, channel},
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

const TERMINAL_FONT_FAMILY: &str = "JetBrainsMono Nerd Font Mono";
const TERMINAL_FONT_SIZE_PX: f32 = 16.0;
const FEEDBACK_CAPTURE_KEY: &str = "f12";

fn terminal_font_features() -> FontFeatures {
    FontFeatures(Arc::new(vec![
        ("calt".to_string(), 0),
        ("liga".to_string(), 0),
        ("kern".to_string(), 0),
    ]))
}

fn terminal_font_fallbacks() -> FontFallbacks {
    FontFallbacks::from_fonts(vec![
        TERMINAL_FONT_FAMILY.to_string(),
        "Consolas".to_string(),
        "Cascadia Mono".to_string(),
        "DejaVu Sans Mono".to_string(),
        "Noto Sans Mono".to_string(),
        "JetBrains Mono".to_string(),
        "Fira Mono".to_string(),
        "Sarasa Mono SC".to_string(),
        "Sarasa Term SC".to_string(),
        "Sarasa Mono J".to_string(),
        "Noto Sans Mono CJK SC".to_string(),
        "Noto Sans Mono CJK JP".to_string(),
        "Source Han Mono SC".to_string(),
        "WenQuanYi Zen Hei Mono".to_string(),
        "Apple Color Emoji".to_string(),
        "Noto Color Emoji".to_string(),
        "Segoe UI Emoji".to_string(),
    ])
}

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

fn rgb_hex(rgb: RgbColor) -> RgbHex {
    RgbHex::new(rgb.r, rgb.g, rgb.b)
}

fn underline_name(underline: Underline) -> &'static str {
    match underline {
        Underline::None => "none",
        Underline::Single => "single",
        Underline::Double => "double",
        Underline::Curly => "curly",
        Underline::Dotted => "dotted",
        Underline::Dashed => "dashed",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RowTextStyle {
    fg: RgbColor,
    bg: Option<RgbColor>,
    default_bg: RgbColor,
    bold: bool,
    italic: bool,
    underline: Underline,
    strikethrough: bool,
}

struct RowSegment {
    start_col: u16,
    columns: u16,
    text: String,
    style: RowTextStyle,
}

impl RowSegment {
    fn new(start_col: u16, columns: u16, text: String, style: RowTextStyle) -> Self {
        Self {
            start_col,
            columns,
            text,
            style,
        }
    }
}

fn mix_rgb(a: RgbColor, b: RgbColor, ratio: f32) -> RgbColor {
    let t = ratio.clamp(0.0, 1.0);
    let blend = |lhs: u8, rhs: u8| -> u8 {
        ((lhs as f32 * (1.0 - t)) + (rhs as f32 * t))
            .round()
            .clamp(0.0, 255.0) as u8
    };

    RgbColor {
        r: blend(a.r, b.r),
        g: blend(a.g, b.g),
        b: blend(a.b, b.b),
    }
}

fn rgb_to_hsv(rgb: RgbColor) -> (f32, f32, f32) {
    let r = rgb.r as f32 / 255.0;
    let g = rgb.g as f32 / 255.0;
    let b = rgb.b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };

    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    (hue, saturation, max)
}

fn bold_display_palette_color(rgb: RgbColor, base_bg: RgbColor) -> RgbColor {
    let (hue, saturation, value) = rgb_to_hsv(rgb);

    if saturation < 0.16 || value < 0.2 {
        return if relative_luminance(base_bg) < 0.35 {
            RgbColor {
                r: 230,
                g: 237,
                b: 243,
            }
        } else {
            RgbColor {
                r: 30,
                g: 41,
                b: 59,
            }
        };
    }

    match hue {
        h if !(15.0..345.0).contains(&h) => RgbColor {
            r: 255,
            g: 123,
            b: 114,
        },
        h if h < 45.0 => RgbColor {
            r: 255,
            g: 184,
            b: 108,
        },
        h if h < 70.0 => RgbColor {
            r: 229,
            g: 192,
            b: 123,
        },
        h if h < 150.0 => RgbColor {
            r: 152,
            g: 195,
            b: 121,
        },
        h if h < 210.0 => RgbColor {
            r: 86,
            g: 212,
            b: 221,
        },
        h if h < 270.0 => RgbColor {
            r: 97,
            g: 175,
            b: 239,
        },
        _ => RgbColor {
            r: 198,
            g: 120,
            b: 221,
        },
    }
}

fn relative_luminance(rgb: RgbColor) -> f32 {
    fn channel(value: u8) -> f32 {
        let normalized = value as f32 / 255.0;
        if normalized <= 0.03928 {
            normalized / 12.92
        } else {
            ((normalized + 0.055) / 1.055).powf(2.4)
        }
    }

    0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b)
}

fn contrast_ratio(a: RgbColor, b: RgbColor) -> f32 {
    let a_lum = relative_luminance(a);
    let b_lum = relative_luminance(b);
    let lighter = a_lum.max(b_lum);
    let darker = a_lum.min(b_lum);
    (lighter + 0.05) / (darker + 0.05)
}

fn emphasized_bold_colors(style: RowTextStyle) -> (RgbColor, Option<RgbColor>) {
    let base_bg = style.bg.unwrap_or(style.default_bg);
    let mut fg = bold_display_palette_color(style.fg, base_bg);
    let target = if relative_luminance(base_bg) < 0.35 {
        RgbColor {
            r: 255,
            g: 255,
            b: 255,
        }
    } else {
        RgbColor { r: 0, g: 0, b: 0 }
    };

    if contrast_ratio(fg, base_bg) < 7.0 {
        for ratio in [0.55_f32, 0.7, 0.82, 0.9] {
            let candidate = mix_rgb(fg, target, ratio);
            fg = candidate;
            if contrast_ratio(fg, base_bg) >= 7.0 {
                break;
            }
        }
    }

    (fg, style.bg)
}

fn resolved_render_style(style: RowTextStyle) -> (RgbColor, Option<RgbColor>, FontWeight) {
    if style.bold {
        let (fg, bg) = emphasized_bold_colors(style);
        (fg, bg, FontWeight::BOLD)
    } else {
        (style.fg, style.bg, FontWeight::NORMAL)
    }
}

fn text_run_for_style(base_style: &TextStyle, style: RowTextStyle, len: usize) -> TextRun {
    let mut run_style = base_style.clone();
    let (fg, _bg, font_weight) = resolved_render_style(style);
    run_style.color = rgb_to_rgba(fg).into();
    run_style.background_color = None;
    run_style.font_weight = font_weight;
    run_style.font_style = if style.italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    run_style.underline = match style.underline {
        Underline::None => None,
        Underline::Curly => Some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(rgb_to_rgba(fg).into()),
            wavy: true,
        }),
        _ => Some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(rgb_to_rgba(fg).into()),
            wavy: false,
        }),
    };
    run_style.strikethrough = style.strikethrough.then_some(StrikethroughStyle {
        thickness: px(1.0),
        color: Some(rgb_to_rgba(fg).into()),
    });
    run_style.to_run(len)
}

fn segment_needs_own_layout(segment: &str, columns: u16) -> bool {
    columns != 1 || !segment.is_ascii()
}

fn push_row_segment(
    segments: &mut Vec<RowSegment>,
    pending: &mut Option<RowSegment>,
    start_col: u16,
    columns: u16,
    style: RowTextStyle,
    text: String,
) {
    if text.is_empty() {
        return;
    }

    let isolate = segment_needs_own_layout(&text, columns);
    if isolate {
        if let Some(segment) = pending.take() {
            segments.push(segment);
        }
        segments.push(RowSegment::new(start_col, columns, text, style));
        return;
    }

    if let Some(segment) = pending.as_mut()
        && segment.style == style
        && segment.start_col + segment.columns == start_col
    {
        segment.columns += columns;
        segment.text.push_str(&text);
        return;
    }

    if let Some(segment) = pending.take() {
        segments.push(segment);
    }
    *pending = Some(RowSegment::new(start_col, columns, text, style));
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_feedback_capture_shortcut(event) {
            self.capture_feedback(window, cx);
            return;
        }

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
            .set_utf8(printable_text.as_deref())
            .set_composing(false);

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

    fn is_feedback_capture_shortcut(&self, event: &KeyDownEvent) -> bool {
        let modifiers = &event.keystroke.modifiers;
        modifiers.control
            && modifiers.shift
            && event
                .keystroke
                .key
                .eq_ignore_ascii_case(FEEDBACK_CAPTURE_KEY)
    }

    fn capture_feedback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.process_output(cx);

        let capture = match self.build_feedback_capture() {
            Ok(capture) => capture,
            Err(err) => {
                eprintln!("Feedback capture failed while snapshotting terminal state: {err:?}");
                return;
            }
        };

        match feedback::write_capture(&capture, window) {
            Ok(paths) => {
                if let Some(png_path) = &paths.png_path {
                    eprintln!(
                        "Feedback capture saved to {} (json: {}, png: {})",
                        paths.directory.display(),
                        paths.json_path.display(),
                        png_path.display()
                    );
                } else if let Some(err) = &paths.pixel_capture_error {
                    eprintln!(
                        "Feedback capture saved JSON to {} but pixel capture failed: {}",
                        paths.json_path.display(),
                        err
                    );
                } else {
                    eprintln!("Feedback capture saved to {}", paths.json_path.display());
                }
            }
            Err(err) => eprintln!("Feedback capture write failed: {err}"),
        }
    }

    fn build_feedback_capture(&mut self) -> crate::ghostty::Result<TerminalCapture> {
        let snapshot = self.render_state.update(&self.terminal)?;
        let colors = snapshot.colors()?;

        let mut rows = Vec::new();
        let mut row_it = self.row_iterator.update(&snapshot)?;
        let mut row_idx = 0u16;
        while let Some(row) = row_it.next() {
            let mut row_text = String::new();
            let mut cells = Vec::new();
            let mut cell_it = self.cell_iterator.update(row)?;
            let mut col_idx = 0u16;
            while let Some(cell) = cell_it.next() {
                let width = cell.width()?;
                let advance = width.column_advance();
                let graphemes_len = cell.graphemes_len()?;
                if graphemes_len == 0
                    || matches!(width, CellWidth::SpacerTail | CellWidth::SpacerHead)
                {
                    col_idx += advance;
                    continue;
                }

                let text: String = cell.graphemes()?.into_iter().collect();
                row_text.push_str(&text);

                let fg = cell.fg_color()?.unwrap_or(colors.foreground);
                let bg = cell.bg_color()?;
                let style = cell.style()?;
                cells.push(CaptureCell {
                    col: col_idx,
                    text,
                    fg: rgb_hex(fg),
                    bg: bg.map(rgb_hex),
                    bold: style.bold,
                    italic: style.italic,
                    underline: underline_name(style.underline).to_string(),
                    inverse: style.inverse,
                    strikethrough: style.strikethrough,
                });
                col_idx += advance;
            }

            rows.push(CaptureRow {
                index: row_idx,
                text: row_text,
                cells,
            });
            row_idx += 1;
        }

        Ok(TerminalCapture {
            captured_unix_ms: feedback::unix_timestamp_ms(),
            terminal_size: GridSize {
                cols: self.size.0,
                rows: self.size.1,
            },
            cell_size_px: SizePx {
                width: self.cell_size.0.into(),
                height: self.cell_size.1.into(),
            },
            font: FontCapture {
                family: TERMINAL_FONT_FAMILY.to_string(),
                size_px: TERMINAL_FONT_SIZE_PX,
            },
            colors: CaptureColors {
                foreground: rgb_hex(colors.foreground),
                background: rgb_hex(colors.background),
                cursor: colors.cursor.map(rgb_hex),
            },
            cursor: snapshot.cursor_viewport()?.map(|cursor| CaptureCursor {
                x: cursor.x,
                y: cursor.y,
            }),
            rows,
        })
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
                    '0' => Key::Digit0,
                    '1' => Key::Digit1,
                    '2' => Key::Digit2,
                    '3' => Key::Digit3,
                    '4' => Key::Digit4,
                    '5' => Key::Digit5,
                    '6' => Key::Digit6,
                    '7' => Key::Digit7,
                    '8' => Key::Digit8,
                    '9' => Key::Digit9,
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
                    .child("Failed to update render state");
            }
        };

        let colors = match snapshot.colors() {
            Ok(c) => c,
            Err(_) => return div().size_full().bg(self.theme.background),
        };

        let cell_size = self.cell_size;
        let mut elements: Vec<gpui::AnyElement> = Vec::new();
        let mut base_text_style = window.text_style();
        base_text_style.font_family = TERMINAL_FONT_FAMILY.into();
        base_text_style.font_features = terminal_font_features();
        base_text_style.font_fallbacks = Some(terminal_font_fallbacks());
        base_text_style.font_size = px(TERMINAL_FONT_SIZE_PX).into();
        base_text_style.line_height = cell_size.1.into();
        base_text_style.white_space = WhiteSpace::Nowrap;

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

            let mut row_segments = Vec::new();
            let mut pending_segment = None;
            let mut col_idx = 0u16;
            while let Some(cell) = cell_it.next() {
                let width = match cell.width() {
                    Ok(width) => width,
                    Err(_) => continue,
                };
                let advance = width.column_advance();
                let start_col = col_idx;
                col_idx += advance;
                let graphemes_len = match cell.graphemes_len() {
                    Ok(n) => n,
                    Err(_) => {
                        if advance > 0 {
                            push_row_segment(
                                &mut row_segments,
                                &mut pending_segment,
                                start_col,
                                advance,
                                RowTextStyle {
                                    fg: colors.foreground,
                                    bg: None,
                                    default_bg: colors.background,
                                    bold: false,
                                    italic: false,
                                    underline: Underline::None,
                                    strikethrough: false,
                                },
                                " ".repeat(advance as usize),
                            );
                        }
                        continue;
                    }
                };

                if matches!(width, CellWidth::SpacerTail | CellWidth::SpacerHead) {
                    continue;
                }

                let fg = cell.fg_color().ok().flatten().unwrap_or(colors.foreground);
                let bg = cell.bg_color().ok().flatten();
                let style = match cell.style() {
                    Ok(s) => s,
                    Err(_) => {
                        push_row_segment(
                            &mut row_segments,
                            &mut pending_segment,
                            start_col,
                            advance.max(1),
                            RowTextStyle {
                                fg,
                                bg,
                                default_bg: colors.background,
                                bold: false,
                                italic: false,
                                underline: Underline::None,
                                strikethrough: false,
                            },
                            " ".repeat(advance.max(1) as usize),
                        );
                        continue;
                    }
                };

                let (fg_color, bg_color, has_bg) = if style.inverse {
                    (fg, bg.unwrap_or(colors.background), true)
                } else {
                    (fg, bg.unwrap_or(colors.background), bg.is_some())
                };

                let segment = if graphemes_len == 0 {
                    " ".repeat(advance.max(1) as usize)
                } else {
                    match cell.graphemes() {
                        Ok(g) => g.into_iter().collect(),
                        Err(_) => " ".repeat(advance.max(1) as usize),
                    }
                };
                push_row_segment(
                    &mut row_segments,
                    &mut pending_segment,
                    start_col,
                    advance.max(1),
                    RowTextStyle {
                        fg: fg_color,
                        bg: (has_bg || style.inverse).then_some(bg_color),
                        default_bg: colors.background,
                        bold: style.bold,
                        italic: style.italic,
                        underline: style.underline,
                        strikethrough: style.strikethrough,
                    },
                    segment,
                );
            }

            if let Some(segment) = pending_segment.take() {
                row_segments.push(segment);
            }

            for segment in row_segments {
                let (x, y) = cell_position(row_idx, segment.start_col, cell_size);
                let segment_width = cell_size.0 * segment.columns as f32;
                let segment_len = segment.text.len();
                let (_, segment_bg, _) = resolved_render_style(segment.style);
                let segment_div = div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(segment_width)
                    .h(cell_size.1)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(TERMINAL_FONT_SIZE_PX))
                    .font_family(TERMINAL_FONT_FAMILY)
                    .line_height(cell_size.1)
                    .when_some(segment_bg, |div, bg| div.bg(rgb_to_rgba(bg)))
                    .child(
                        StyledText::new(segment.text).with_runs(vec![text_run_for_style(
                            &base_text_style,
                            segment.style,
                            segment_len,
                        )]),
                    );
                elements.push(segment_div.into_any_element());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_style_survives_box_emoji_prompt_segment() {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 32,
            rows: 4,
            max_scrollback: 100,
        })
        .expect("terminal");
        terminal.resize(32, 4, 10, 20).expect("resize");
        terminal.vt_write("📦 \u{1b}[1mrepo\u{1b}[0m".as_bytes());

        let mut render_state = RenderState::new().expect("render state");
        let snapshot = render_state.update(&terminal).expect("snapshot");
        let mut row_iterator = RowIterator::new().expect("row iterator");
        let mut cell_iterator = CellIterator::new().expect("cell iterator");

        let mut rows = row_iterator.update(&snapshot).expect("rows");
        let row = rows.next().expect("first row");
        let mut cells = cell_iterator.update(row).expect("cells");

        let mut letters = Vec::new();
        while let Some(cell) = cells.next() {
            let text: String = cell.graphemes().expect("graphemes").into_iter().collect();
            if text.is_empty() {
                continue;
            }

            if matches!(text.as_str(), "r" | "e" | "p" | "o") {
                letters.push((text, cell.style().expect("style").bold));
            }
        }

        assert_eq!(
            letters,
            vec![
                ("r".to_string(), true),
                ("e".to_string(), true),
                ("p".to_string(), true),
                ("o".to_string(), true),
            ]
        );
    }

    #[test]
    fn box_emoji_advances_two_columns() {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 32,
            rows: 4,
            max_scrollback: 100,
        })
        .expect("terminal");
        terminal.resize(32, 4, 10, 20).expect("resize");
        terminal.vt_write("x📦y".as_bytes());

        let mut render_state = RenderState::new().expect("render state");
        let snapshot = render_state.update(&terminal).expect("snapshot");
        let mut row_iterator = RowIterator::new().expect("row iterator");
        let mut cell_iterator = CellIterator::new().expect("cell iterator");

        let mut rows = row_iterator.update(&snapshot).expect("rows");
        let row = rows.next().expect("first row");
        let mut cells = cell_iterator.update(row).expect("cells");

        let mut positions = Vec::new();
        let mut col_idx = 0u16;
        while let Some(cell) = cells.next() {
            let width = cell.width().expect("width");
            let advance = width.column_advance();
            let text: String = cell.graphemes().expect("graphemes").into_iter().collect();

            if !text.is_empty() && !matches!(width, CellWidth::SpacerTail | CellWidth::SpacerHead) {
                positions.push((text, col_idx, width));
            }

            col_idx += advance;
        }

        assert_eq!(
            positions,
            vec![
                ("x".to_string(), 0, CellWidth::Narrow),
                ("📦".to_string(), 1, CellWidth::Wide),
                ("y".to_string(), 3, CellWidth::Narrow),
            ]
        );
    }
}
