//! Terminal Widget
//!
//! GPUI component that owns terminal state and wires shell I/O, input encoding,
//! rendering, and feedback capture together.

mod capture;
mod input;
mod pty;
mod render;

use crate::feedback;
use crate::ghostty::{
    RenderState, Terminal, TerminalOptions,
    key::{Action, Encoder, Event},
    render::{CellIterator, RowIterator},
    style::RgbColor,
};
use crate::pane_container::shortcut_action;
use crate::shell::PtySize;
use gpui::{
    Context, FocusHandle, KeyDownEvent, KeyUpEvent, MouseDownEvent, Pixels, Size, Task, Timer,
    Window, px,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use pty::{OUTPUT_DRAIN_BUDGET, PtyCommand, PtyEvent, PtyWorker};

pub(super) const TERMINAL_FONT_FAMILY: &str = "JetBrainsMono Nerd Font Mono";
pub(super) const TERMINAL_FONT_SIZE_PX: f32 = 16.0;

const FEEDBACK_CAPTURE_KEY: &str = "f12";

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
            shell: default_shell(),
            initial_rows: 24,
            initial_cols: 80,
            scrollback: 1000,
            cursor_style: CursorStyle::Line,
            cursor_blink: true,
            blink_interval: Duration::from_millis(500),
        }
    }
}

#[cfg(windows)]
fn default_shell() -> String {
    "pwsh.exe".to_string()
}

#[cfg(unix)]
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

#[cfg(not(any(windows, unix)))]
fn default_shell() -> String {
    String::new()
}

pub struct TerminalWidget {
    terminal: Terminal<'static, 'static>,
    key_encoder: Encoder<'static>,
    key_event: Event<'static>,
    render_state: RenderState<'static>,
    row_iterator: RowIterator<'static>,
    cell_iterator: CellIterator<'static>,
    config: TerminalConfig,
    pty_tx: Option<flume::Sender<PtyCommand>>,
    exit_signal_tx: Option<flume::Sender<()>>,
    exit_flag: Arc<AtomicBool>,
    pty_worker: Option<PtyWorker>,
    output_task: Task<()>,
    cursor_blink_task: Task<()>,
    focus_handle: FocusHandle,
    cursor_blink_phase: bool,
    size: (u16, u16),
    layout_size: Option<Size<Pixels>>,
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
            foreground: gpui::rgb(0xc0c0c0),
            background: gpui::rgb(0x000000),
            cursor: gpui::rgb(0xffffff),
            selection: gpui::rgb(0x3d3d3d),
            palette: [
                gpui::rgb(0x000000),
                gpui::rgb(0xcd0000),
                gpui::rgb(0x00cd00),
                gpui::rgb(0xcdcd00),
                gpui::rgb(0x0000ee),
                gpui::rgb(0xcd00cd),
                gpui::rgb(0x00cdcd),
                gpui::rgb(0xe5e5e5),
                gpui::rgb(0x7f7f7f),
                gpui::rgb(0xff0000),
                gpui::rgb(0x00ff00),
                gpui::rgb(0xffff00),
                gpui::rgb(0x5c5cff),
                gpui::rgb(0xff00ff),
                gpui::rgb(0x00ffff),
                gpui::rgb(0xffffff),
            ],
        }
    }
}

impl TerminalWidget {
    pub fn new(config: TerminalConfig, cx: &mut Context<Self>) -> Self {
        let theme = TerminalTheme::default();
        let exit_flag = Arc::new(AtomicBool::new(false));

        let mut terminal = Terminal::new(TerminalOptions {
            cols: config.initial_cols,
            rows: config.initial_rows,
            max_scrollback: config.scrollback,
        })
        .expect("Failed to create terminal");

        #[cfg(any(windows, unix))]
        let (pty_worker, pty_event_rx, pty_tx) = {
            match PtyWorker::spawn(
                config.shell.clone(),
                config.initial_rows,
                config.initial_cols,
                Arc::clone(&exit_flag),
            ) {
                Ok((worker, event_rx)) => {
                    let command_tx = worker.command_tx();
                    (Some(worker), Some(event_rx), Some(command_tx))
                }
                Err(err) => {
                    eprintln!("Failed to spawn shell: {err}");
                    exit_flag.store(true, Ordering::Relaxed);
                    (None, None, None)
                }
            }
        };

        #[cfg(not(any(windows, unix)))]
        let (pty_worker, pty_event_rx, pty_tx) = {
            exit_flag.store(true, Ordering::Relaxed);
            (None, None, None)
        };

        let pty_response_tx = pty_tx.clone();
        terminal
            .on_pty_write(move |_terminal, data| {
                if let Some(tx) = &pty_response_tx {
                    let _ = tx.send(PtyCommand::Write(data.to_vec()));
                }
            })
            .expect("Failed to configure terminal PTY responses");
        terminal
            .set_default_fg_color(Some(rgba_to_rgb(theme.foreground)))
            .and_then(|terminal| terminal.set_default_bg_color(Some(rgba_to_rgb(theme.background))))
            .and_then(|terminal| terminal.set_default_cursor_color(Some(rgba_to_rgb(theme.cursor))))
            .and_then(|terminal| {
                terminal.set_default_color_palette(Some(terminal_palette(theme.palette)))
            })
            .expect("Failed to configure terminal default colors");

        let render_state = RenderState::new().expect("Failed to create render state");
        let row_iterator = RowIterator::new().expect("Failed to create row iterator");
        let cell_iterator = CellIterator::new().expect("Failed to create cell iterator");
        let key_encoder = Encoder::new().expect("Failed to create key encoder");
        let key_event = Event::new().expect("Failed to create key event");

        let size = (config.initial_cols, config.initial_rows);

        let has_exited = exit_flag.load(Ordering::Relaxed);

        let mut widget = Self {
            terminal,
            key_encoder,
            key_event,
            render_state,
            row_iterator,
            cell_iterator,
            config,
            pty_tx,
            exit_signal_tx: None,
            exit_flag,
            pty_worker,
            output_task: Task::ready(()),
            cursor_blink_task: Task::ready(()),
            focus_handle: cx.focus_handle(),
            cursor_blink_phase: true,
            size,
            layout_size: None,
            cell_size: (px(9.6), px(19.2)),
            theme,
            has_exited,
        };

        if let Some(event_rx) = pty_event_rx {
            widget.start_output_task(event_rx, cx);
        }
        widget.schedule_cursor_blink(cx);
        widget
    }

    pub fn set_exit_flag(&mut self, flag: Arc<AtomicBool>) {
        self.exit_flag = flag;
    }

    pub fn has_exited(&self) -> bool {
        self.has_exited || self.exit_flag.load(Ordering::Relaxed)
    }

    pub fn set_exit_signal(&mut self, tx: flume::Sender<()>) {
        self.exit_signal_tx = Some(tx);
        if self.has_exited() {
            self.mark_exited();
        }
    }

    pub fn request_focus(&self, window: &mut Window) {
        self.focus_handle.focus(window);
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    fn schedule_cursor_blink(&mut self, cx: &mut Context<Self>) {
        if !self.config.cursor_blink {
            self.cursor_blink_phase = true;
            self.cursor_blink_task = Task::ready(());
            return;
        }

        let interval = self.config.blink_interval;
        self.cursor_blink_task = cx.spawn(async move |this, cx| {
            Timer::after(interval).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    if !this.config.cursor_blink {
                        this.cursor_blink_phase = true;
                        this.cursor_blink_task = Task::ready(());
                        cx.notify();
                        return;
                    }

                    this.cursor_blink_phase = !this.cursor_blink_phase;
                    cx.notify();
                    this.schedule_cursor_blink(cx);
                })
                .ok();
            }
        });
    }

    fn start_output_task(&mut self, event_rx: flume::Receiver<PtyEvent>, cx: &mut Context<Self>) {
        self.output_task = cx.spawn(async move |this, cx| {
            while let Ok(first_event) = event_rx.recv_async().await {
                let mut events = vec![first_event];
                let mut drained_bytes = events.iter().map(PtyEvent::len).sum::<usize>();

                while drained_bytes < OUTPUT_DRAIN_BUDGET {
                    match event_rx.try_recv() {
                        Ok(event) => {
                            drained_bytes += event.len();
                            events.push(event);
                        }
                        Err(flume::TryRecvError::Empty) => break,
                        Err(flume::TryRecvError::Disconnected) => break,
                    }
                }

                let Some(this) = this.upgrade() else {
                    break;
                };

                this.update(cx, |this, cx| {
                    for event in events {
                        this.apply_pty_event(event);
                    }
                    if this.exit_flag.load(Ordering::Relaxed) {
                        this.mark_exited();
                    }
                    cx.notify();
                })
                .ok();
            }
        });
    }

    fn apply_pty_event(&mut self, event: PtyEvent) {
        match event {
            PtyEvent::Output(data) => self.terminal.vt_write(&data),
            PtyEvent::Exited => self.mark_exited(),
        }
    }

    fn send_pty_command(&mut self, command: PtyCommand) {
        if let Some(tx) = &self.pty_tx
            && tx.send(command).is_err()
        {
            self.mark_exited();
        }
    }

    fn mark_exited(&mut self) {
        self.exit_flag.store(true, Ordering::Relaxed);
        self.has_exited = true;
        self.signal_exit();
    }

    fn signal_exit(&self) {
        if let Some(tx) = &self.exit_signal_tx {
            let _ = tx.send(());
        }
    }

    fn reset_cursor_blink(&mut self, cx: &mut Context<Self>) {
        if self.config.cursor_blink {
            self.cursor_blink_phase = true;
            cx.notify();
            self.schedule_cursor_blink(cx);
        }
    }

    fn calculate_dimensions(&self, size: Size<Pixels>) -> (u16, u16) {
        let cols = (size.width / self.cell_size.0).floor() as u16;
        let rows = (size.height / self.cell_size.1).floor() as u16;
        (cols.max(1), rows.max(1))
    }

    fn resize_to_size(&mut self, size: Size<Pixels>, cx: &mut Context<Self>) {
        let (cols, rows) = self.calculate_dimensions(size);
        if cols != self.size.0 || rows != self.size.1 {
            let cell_width: f32 = self.cell_size.0.into();
            let cell_height: f32 = self.cell_size.1.into();
            if self
                .terminal
                .resize(cols, rows, cell_width as u32, cell_height as u32)
                .is_ok()
            {
                self.size = (cols, rows);
                self.send_pty_command(PtyCommand::Resize(PtySize::new(rows, cols)));
                cx.notify();
            }
        }
    }

    fn update_layout_size(&mut self, size: Size<Pixels>, cx: &mut Context<Self>) {
        if self.layout_size == Some(size) {
            return;
        }

        self.layout_size = Some(size);
        self.resize_to_size(size, cx);
        cx.notify();
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dispatch_app_shortcut(&event.keystroke, window, cx) {
            return;
        }

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

    fn handle_key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_app_shortcut(&event.keystroke) {
            window.prevent_default();
            cx.stop_propagation();
            return;
        }

        self.send_encoded_key(Action::Release, &event.keystroke, cx);
    }

    fn dispatch_app_shortcut(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(action) = shortcut_action(keystroke) else {
            return false;
        };

        window.dispatch_action(action, cx);
        window.prevent_default();
        cx.stop_propagation();
        true
    }

    fn is_app_shortcut(&self, keystroke: &gpui::Keystroke) -> bool {
        shortcut_action(keystroke).is_some()
    }

    fn send_encoded_key(
        &mut self,
        action: Action,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) {
        if let Some(vt_bytes) = input::encode_key_event(
            &mut self.key_encoder,
            &mut self.key_event,
            &self.terminal,
            action,
            keystroke,
        ) {
            self.send_pty_command(PtyCommand::Write(vt_bytes));
        }
        self.reset_cursor_blink(cx);
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

    fn capture_feedback(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
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

    fn handle_mouse_down(
        &mut self,
        _event: &MouseDownEvent,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
    }
}

impl Drop for TerminalWidget {
    fn drop(&mut self) {
        self.output_task = Task::ready(());
        self.cursor_blink_task = Task::ready(());
        self.pty_tx = None;
        if let Some(worker) = self.pty_worker.as_mut() {
            worker.shutdown();
        }
        self.pty_worker = None;
    }
}

pub(super) fn rgb_to_rgba(rgb: RgbColor) -> gpui::Rgba {
    gpui::rgb((rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32)
}

fn rgba_to_rgb(rgba: gpui::Rgba) -> RgbColor {
    RgbColor {
        r: (rgba.r * 255.0).round().clamp(0.0, 255.0) as u8,
        g: (rgba.g * 255.0).round().clamp(0.0, 255.0) as u8,
        b: (rgba.b * 255.0).round().clamp(0.0, 255.0) as u8,
    }
}

fn terminal_palette(theme_palette: [gpui::Rgba; 16]) -> [RgbColor; 256] {
    let mut palette = [RgbColor { r: 0, g: 0, b: 0 }; 256];
    for (index, color) in theme_palette.into_iter().enumerate() {
        palette[index] = rgba_to_rgb(color);
    }

    let levels = [0, 95, 135, 175, 215, 255];
    let mut index = 16;
    for r in levels {
        for g in levels {
            for b in levels {
                palette[index] = RgbColor { r, g, b };
                index += 1;
            }
        }
    }

    for gray_index in 0..24 {
        let value = 8 + gray_index * 10;
        palette[232 + gray_index as usize] = RgbColor {
            r: value,
            g: value,
            b: value,
        };
    }

    palette
}
