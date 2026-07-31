//! Terminal Widget
//!
//! GPUI component that owns terminal state and wires shell I/O, input encoding,
//! rendering, and feedback capture together.

mod capture;
mod graphics;
mod input;
mod pty;
mod render;
mod search;

use crate::action::{
    ActionBinding, AppAction, DispatchAppAction, PromptDirection, chord_for_keystroke,
    default_action_bindings,
};
use crate::feedback;
use crate::ghostty::{
    ClipboardLocation, ClipboardWrite, ClipboardWriteResult, RenderState, Scrollbar,
    SearchDirection, SearchProgress, SelectionDrag, SelectionGeometry, SelectionPoint,
    SelectionPress, Terminal, TerminalOptions, ViewportScroll,
    key::{Action, Encoder, Event},
    mouse::{Action as MouseAction, Button as MouseButton, Encoder as MouseEncoder},
    mouse::{Event as MouseEvent, Geometry as MouseGeometry},
    paste,
    render::{CellIterator, RowIterator},
    style::{Palette, RgbColor},
};
use crate::profile::LaunchSpec;
use crate::shell::PtySize;
use gpui::{
    Bounds, ClipboardItem, Context, EventEmitter, FocusHandle, KeyDownEvent, KeyUpEvent, Modifiers,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, Size, Task,
    Timer, Window, px,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use pty::{OUTPUT_DRAIN_BUDGET, PtyCommand, PtyEvent, PtyWorker};

pub const DEFAULT_TERMINAL_FONT_FAMILY: &str = "JetBrainsMono Nerd Font Mono";
pub const DEFAULT_TERMINAL_FONT_SIZE_PX: f32 = 16.0;

const FEEDBACK_CAPTURE_KEY: &str = "f12";
const CLICK_REPEAT_INTERVAL_NS: u64 = 500_000_000;
const SCROLLBAR_MIN_THUMB_PX: f32 = 24.0;
const MAX_TERMINAL_CLIPBOARD_BYTES: usize = 1024 * 1024;

/// Cursor style options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    #[default]
    Block,
    Line,
    Underline,
}

/// Policy for clipboard writes requested by terminal output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalClipboardPolicy {
    #[default]
    Deny,
    AllowText,
}

/// Terminal widget configuration
#[derive(Debug, Clone)]
pub struct TerminalConfig {
    pub launch: LaunchSpec,
    pub initial_rows: u16,
    pub initial_cols: u16,
    pub scrollback: usize,
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    pub blink_interval: Duration,
    pub terminal_clipboard_policy: TerminalClipboardPolicy,
    pub font_family: String,
    pub font_size_px: f32,
    pub theme: TerminalTheme,
    pub action_bindings: Vec<ActionBinding>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            launch: LaunchSpec::default_shell(),
            initial_rows: 30,
            initial_cols: 100,
            scrollback: 10000,
            cursor_style: CursorStyle::Line,
            cursor_blink: true,
            blink_interval: Duration::from_millis(500),
            terminal_clipboard_policy: TerminalClipboardPolicy::Deny,
            font_family: DEFAULT_TERMINAL_FONT_FAMILY.to_string(),
            font_size_px: DEFAULT_TERMINAL_FONT_SIZE_PX,
            theme: TerminalTheme::default(),
            action_bindings: default_action_bindings(),
        }
    }
}

pub struct TerminalWidget {
    terminal: Terminal,
    key_encoder: Encoder,
    key_event: Event,
    mouse_encoder: MouseEncoder,
    mouse_event: MouseEvent,
    render_state: RenderState,
    row_iterator: RowIterator,
    cell_iterator: CellIterator,
    graphics_renderer: graphics::GraphicsRenderer,
    search: Option<search::SearchOverlay>,
    search_task: Task<()>,
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
    layout_bounds: Option<Bounds<Pixels>>,
    cell_size: (Pixels, Pixels),
    pending_scroll_y: f32,
    selecting: bool,
    reported_button: Option<MouseButton>,
    scrollbar_dragging: bool,
    pending_paste: Option<String>,
    terminal_clipboard_writes: Rc<RefCell<Vec<String>>>,
    terminal_effects: Rc<RefCell<PendingTerminalEffects>>,
    reported_title: Option<String>,
    reported_working_directory: Option<Option<String>>,
    semantic_commands_available: bool,
    theme: TerminalTheme,
    has_exited: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    TitleChanged(Option<String>),
    WorkingDirectoryChanged(Option<String>),
    Bell,
}

impl EventEmitter<TerminalEvent> for TerminalWidget {}

#[derive(Default)]
struct PendingTerminalEffects {
    title: Option<crate::ghostty::Result<Option<String>>>,
    working_directory: Option<crate::ghostty::Result<Option<String>>>,
    bell: bool,
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
        let theme = config.theme.clone();
        let exit_flag = Arc::new(AtomicBool::new(false));

        let mut terminal = Terminal::new(TerminalOptions {
            cols: config.initial_cols,
            rows: config.initial_rows,
            max_scrollback: config.scrollback,
        })
        .expect("Failed to create terminal");
        terminal
            .enable_direct_graphics(graphics::DIRECT_GRAPHICS_STORAGE_LIMIT)
            .expect("Failed to enable direct terminal graphics");
        let terminal_clipboard_writes = Rc::new(RefCell::new(Vec::new()));
        let terminal_effects = Rc::new(RefCell::new(PendingTerminalEffects::default()));

        #[cfg(any(windows, unix))]
        let (pty_worker, pty_event_rx, pty_tx) = {
            let launch = match crate::shell_integration::prepare_launch(&config.launch) {
                Ok(launch) => launch,
                Err(error) => {
                    eprintln!("Failed to prepare shell integration: {error}");
                    config.launch.clone()
                }
            };
            match PtyWorker::spawn(
                launch,
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
            .on_pty_write(move |data| {
                if let Some(tx) = &pty_response_tx {
                    let _ = tx.send(PtyCommand::Write(data.to_vec()));
                }
            })
            .expect("Failed to configure terminal PTY responses");
        let terminal_clipboard_policy = config.terminal_clipboard_policy;
        let clipboard_writes = Rc::clone(&terminal_clipboard_writes);
        terminal
            .on_clipboard_write(move |write| {
                let Some(text) = accepted_terminal_clipboard_text(terminal_clipboard_policy, write)
                else {
                    return ClipboardWriteResult::Denied;
                };
                clipboard_writes.borrow_mut().push(text);
                ClipboardWriteResult::Success
            })
            .expect("Failed to configure terminal clipboard policy");
        let effects = Rc::clone(&terminal_effects);
        terminal
            .on_title_changed(move |title| effects.borrow_mut().title = Some(title))
            .expect("Failed to configure terminal title updates");
        let effects = Rc::clone(&terminal_effects);
        terminal
            .on_working_directory_changed(move |working_directory| {
                effects.borrow_mut().working_directory = Some(working_directory);
            })
            .expect("Failed to configure terminal working-directory updates");
        let effects = Rc::clone(&terminal_effects);
        terminal
            .on_bell(move || effects.borrow_mut().bell = true)
            .expect("Failed to configure terminal bell updates");
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
        let graphics_renderer =
            graphics::GraphicsRenderer::new().expect("Failed to create graphics renderer");
        let key_encoder = Encoder::new().expect("Failed to create key encoder");
        let key_event = Event::new().expect("Failed to create key event");
        let mouse_encoder = MouseEncoder::new().expect("Failed to create mouse encoder");
        let mouse_event = MouseEvent::new().expect("Failed to create mouse event");

        let size = (config.initial_cols, config.initial_rows);

        let has_exited = exit_flag.load(Ordering::Relaxed);

        let mut widget = Self {
            terminal,
            key_encoder,
            key_event,
            mouse_encoder,
            mouse_event,
            render_state,
            row_iterator,
            cell_iterator,
            graphics_renderer,
            search: None,
            search_task: Task::ready(()),
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
            layout_bounds: None,
            cell_size: (px(9.6), px(19.2)),
            pending_scroll_y: 0.0,
            selecting: false,
            reported_button: None,
            scrollbar_dragging: false,
            pending_paste: None,
            terminal_clipboard_writes,
            terminal_effects,
            reported_title: None,
            reported_working_directory: None,
            semantic_commands_available: false,
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

    pub(crate) fn set_action_bindings(&mut self, bindings: Vec<ActionBinding>) {
        self.config.action_bindings = bindings;
    }

    pub(crate) fn has_selection(&self) -> bool {
        self.terminal
            .selected_text()
            .is_ok_and(|text| text.is_some_and(|text| !text.is_empty()))
    }

    pub(crate) fn reported_local_working_directory(&self) -> Option<PathBuf> {
        self.reported_working_directory
            .as_ref()
            .and_then(Option::as_deref)
            .and_then(crate::shell_integration::local_working_directory)
    }

    pub(crate) fn workspace_working_directory(&self) -> Option<PathBuf> {
        match &self.reported_working_directory {
            Some(Some(report)) => crate::shell_integration::local_working_directory(report),
            Some(None) => None,
            None => self.config.launch.working_directory.clone(),
        }
    }

    pub(crate) fn reported_title(&self) -> Option<&str> {
        self.reported_title.as_deref()
    }

    pub(crate) fn semantic_commands_available(&self) -> bool {
        self.semantic_commands_available
    }

    pub(crate) fn jump_to_prompt(&mut self, direction: PromptDirection, cx: &mut Context<Self>) {
        match self.terminal.jump_to_prompt(direction) {
            Ok(true) => cx.notify(),
            Ok(false) => {}
            Err(error) => eprintln!("Failed to navigate semantic prompts: {error}"),
        }
    }

    pub(crate) fn select_command_output(&mut self, cx: &mut Context<Self>) {
        match self.terminal.select_command_output() {
            Ok(true) => cx.notify(),
            Ok(false) => {}
            Err(error) => eprintln!("Failed to select command output: {error}"),
        }
    }

    pub(crate) fn copy_command_output(&mut self, cx: &mut Context<Self>) {
        match self.terminal.command_output_text() {
            Ok(Some(text)) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Ok(None) => {}
            Err(error) => eprintln!("Failed to copy command output: {error}"),
        }
    }

    pub(crate) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_none() {
            self.search = Some(search::SearchOverlay::default());
        }
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal.stop_search();
        self.search = None;
        self.search_task = Task::ready(());
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn restart_search(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        search.revision = search.revision.wrapping_add(1);
        search.ranges.clear();
        search.active = None;
        search.diagnostic = None;
        search.step_scheduled = false;
        if search.query.is_empty() {
            search.progress = SearchProgress::Complete;
            self.terminal.stop_search();
            self.search_task = Task::ready(());
            cx.notify();
            return;
        }

        match self.terminal.start_search(&search.query) {
            Ok(()) => {
                search.progress = SearchProgress::Pending;
                self.schedule_search_step(cx);
            }
            Err(error) => {
                search.progress = SearchProgress::Complete;
                search.diagnostic = Some(error.to_string());
                cx.notify();
            }
        }
    }

    fn schedule_search_step(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        if search.query.is_empty() || search.step_scheduled {
            return;
        }
        search.step_scheduled = true;
        let revision = search.revision;
        self.search_task = cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(1)).await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                let Some(search) = this.search.as_mut() else {
                    return;
                };
                if search.revision != revision {
                    return;
                }
                search.step_scheduled = false;

                let progress = this.terminal.search_step();
                let ranges = this.terminal.search_ranges();
                let search = this.search.as_mut().expect("search remains open");
                match (progress, ranges) {
                    (Ok(progress), Ok(ranges)) => {
                        search.progress = progress;
                        search.ranges = ranges;
                        if search
                            .active
                            .is_some_and(|active| !search.ranges.contains(&active))
                        {
                            search.active = None;
                        }
                        search.diagnostic = None;
                        cx.notify();
                        if progress == SearchProgress::Pending {
                            this.schedule_search_step(cx);
                        }
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        search.progress = SearchProgress::Complete;
                        search.diagnostic = Some(error.to_string());
                        cx.notify();
                    }
                }
            })
            .ok();
        });
    }

    fn navigate_search(&mut self, direction: SearchDirection, cx: &mut Context<Self>) {
        match self.terminal.search_select(direction) {
            Ok(Some(range)) => {
                self.terminal
                    .scroll_viewport(ViewportScroll::Row(range.start.row as usize));
                if let Some(search) = self.search.as_mut() {
                    search.active = Some(range);
                }
                cx.notify();
            }
            Ok(None) => {}
            Err(error) => {
                if let Some(search) = self.search.as_mut() {
                    search.diagnostic = Some(error.to_string());
                }
                cx.notify();
            }
        }
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
                        this.apply_pty_event(event, cx);
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

    fn apply_pty_event(&mut self, event: PtyEvent, cx: &mut Context<Self>) {
        match event {
            PtyEvent::Output(data) => {
                self.terminal.vt_write(&data);
                if self.search.is_some() {
                    self.schedule_search_step(cx);
                }
                if !self.semantic_commands_available {
                    self.semantic_commands_available =
                        self.terminal.has_semantic_prompt().unwrap_or(false);
                }
                for text in self.terminal_clipboard_writes.borrow_mut().drain(..) {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                self.emit_terminal_effects(cx);
            }
            PtyEvent::Exited => self.mark_exited(),
        }
    }

    fn emit_terminal_effects(&mut self, cx: &mut Context<Self>) {
        let effects = std::mem::take(&mut *self.terminal_effects.borrow_mut());
        if let Some(title) = effects.title {
            match title {
                Ok(title) => {
                    self.reported_title = title.clone();
                    cx.emit(TerminalEvent::TitleChanged(title));
                }
                Err(error) => eprintln!("Ignored invalid terminal title: {error}"),
            }
        }
        if let Some(working_directory) = effects.working_directory {
            match working_directory {
                Ok(working_directory) => {
                    self.reported_working_directory = Some(working_directory.clone());
                    cx.emit(TerminalEvent::WorkingDirectoryChanged(working_directory));
                }
                Err(error) => eprintln!("Ignored invalid terminal working directory: {error}"),
            }
        }
        if effects.bell {
            cx.emit(TerminalEvent::Bell);
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

    fn update_layout_bounds(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.layout_bounds == Some(bounds) {
            return;
        }

        self.layout_bounds = Some(bounds);
        self.resize_to_size(bounds.size, cx);
        cx.notify();
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search.is_some() {
            match event.keystroke.key.as_str() {
                "escape" => self.close_search(window, cx),
                "enter" => {
                    let direction = if event.keystroke.modifiers.shift {
                        SearchDirection::Previous
                    } else {
                        SearchDirection::Next
                    };
                    self.navigate_search(direction, cx);
                }
                "backspace" => {
                    self.search
                        .as_mut()
                        .expect("search remains open")
                        .query
                        .pop();
                    self.restart_search(cx);
                }
                _ if !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt
                    && !event.keystroke.modifiers.platform =>
                {
                    if let Some(text) = &event.keystroke.key_char {
                        let search = self.search.as_mut().expect("search remains open");
                        if search.query.len().saturating_add(text.len())
                            <= search::MAX_SEARCH_QUERY_BYTES
                        {
                            search.query.push_str(text);
                            self.restart_search(cx);
                        }
                    }
                }
                _ => {}
            }
            window.prevent_default();
            cx.stop_propagation();
            return;
        }

        if self.pending_paste.is_some() {
            match event.keystroke.key.as_str() {
                "enter" => self.confirm_pending_paste(cx),
                "escape" => self.cancel_pending_paste(cx),
                _ => {}
            }
            window.prevent_default();
            cx.stop_propagation();
            return;
        }

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
        if self.search.is_some() || self.pending_paste.is_some() {
            window.prevent_default();
            cx.stop_propagation();
            return;
        }

        if self.app_action_for_keystroke(&event.keystroke).is_some() {
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
        let Some(action) = self.app_action_for_keystroke(keystroke).cloned() else {
            return false;
        };

        window.dispatch_action(Box::new(DispatchAppAction { action }), cx);
        window.prevent_default();
        cx.stop_propagation();
        true
    }

    fn app_action_for_keystroke(&self, keystroke: &gpui::Keystroke) -> Option<&AppAction> {
        let chord = chord_for_keystroke(keystroke);
        self.config
            .action_bindings
            .iter()
            .find(|binding| binding.chord == chord)
            .map(|binding| &binding.action)
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
            self.terminal.scroll_viewport(ViewportScroll::Bottom);
            if let Err(error) = self.terminal.clear_selection() {
                eprintln!("Failed to clear terminal selection after typing: {error}");
            }
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
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
        if self.try_open_hyperlink(event, window, cx) {
            return;
        }
        if self.should_report_mouse(&event.modifiers) {
            self.reported_button = ghostty_mouse_button(event.button);
            self.send_mouse_report(
                MouseAction::Press,
                self.reported_button,
                event.position,
                &event.modifiers,
            );
            window.prevent_default();
            cx.stop_propagation();
            return;
        }
        if event.button != gpui::MouseButton::Left {
            return;
        }
        let Some(point) = self.selection_point(event.position) else {
            return;
        };

        let press = SelectionPress {
            point,
            time_ns: monotonic_time_ns(),
            repeat_interval_ns: CLICK_REPEAT_INTERVAL_NS,
            repeat_distance: f64::from(f32::from(self.cell_size.0)),
        };
        match self.terminal.selection_press(press) {
            Ok(()) => {
                self.selecting = true;
                cx.notify();
            }
            Err(error) => {
                self.selecting = false;
                eprintln!("Failed to begin terminal selection: {error}");
            }
        }
    }

    fn handle_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.selecting {
            if !event.dragging() {
                return;
            }
            let Some(point) = self.selection_point(event.position) else {
                return;
            };
            let drag = SelectionDrag {
                point,
                geometry: self.selection_geometry(),
                rectangle: rectangle_selection(&event.modifiers),
            };
            if let Err(error) = self.terminal.selection_drag(drag) {
                eprintln!("Failed to update terminal selection: {error}");
                return;
            }
            cx.notify();
            return;
        }

        if self.should_report_mouse(&event.modifiers) {
            self.reported_button = event.pressed_button.and_then(ghostty_mouse_button);
            self.send_mouse_report(
                MouseAction::Motion,
                self.reported_button,
                event.position,
                &event.modifiers,
            );
        }
    }

    fn handle_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selecting {
            let point = self.selection_point(event.position);
            if let Err(error) = self.terminal.selection_release(point) {
                eprintln!("Failed to finish terminal selection: {error}");
            }
            self.selecting = false;
            cx.notify();
            return;
        }

        if self.reported_button.is_some() || self.should_report_mouse(&event.modifiers) {
            let button = ghostty_mouse_button(event.button).or(self.reported_button);
            self.reported_button = None;
            self.send_mouse_report(
                MouseAction::Release,
                button,
                event.position,
                &event.modifiers,
            );
            window.prevent_default();
            cx.stop_propagation();
        }
    }

    fn handle_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pixel_delta = event.delta.pixel_delta(self.cell_size.1);
        let delta_y: f32 = pixel_delta.y.into();
        let cell_height: f32 = self.cell_size.1.into();
        let wheel_rows = accumulated_scroll_rows(&mut self.pending_scroll_y, delta_y, cell_height);
        if wheel_rows != 0 {
            if self.should_report_mouse(&event.modifiers) {
                let button = if wheel_rows > 0 {
                    MouseButton::Four
                } else {
                    MouseButton::Five
                };
                for _ in 0..wheel_rows.unsigned_abs().min(16) {
                    self.send_mouse_report(
                        MouseAction::Press,
                        Some(button),
                        event.position,
                        &event.modifiers,
                    );
                }
            } else {
                self.terminal
                    .scroll_viewport(wheel_rows_to_viewport_scroll(wheel_rows));
                cx.notify();
            }
        }
        window.prevent_default();
        cx.stop_propagation();
    }

    fn should_report_mouse(&self, modifiers: &Modifiers) -> bool {
        !modifiers.shift && self.terminal.mouse_tracking().unwrap_or(false)
    }

    fn send_mouse_report(
        &mut self,
        action: MouseAction,
        button: Option<MouseButton>,
        position: Point<Pixels>,
        modifiers: &Modifiers,
    ) {
        let Some((x, y)) = self.mouse_surface_position(position) else {
            return;
        };
        self.mouse_event
            .set_action(action)
            .set_button(button)
            .set_mods(input::convert_modifiers(modifiers))
            .set_position(x, y);

        let geometry = self.mouse_geometry();
        let any_button_pressed = self.reported_button.is_some();
        let mut output = Vec::with_capacity(64);
        if let Err(error) = self
            .mouse_encoder
            .set_options_from_terminal(&self.terminal)
            .set_geometry(geometry)
            .set_any_button_pressed(any_button_pressed)
            .encode_to_vec(&self.mouse_event, &mut output)
        {
            eprintln!("Failed to encode terminal mouse input: {error}");
            return;
        }
        if !output.is_empty() {
            self.send_pty_command(PtyCommand::Write(output));
        }
    }

    fn mouse_surface_position(&self, position: Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = self.layout_bounds?;
        let local = position - bounds.origin;
        let width: f32 = bounds.size.width.into();
        let height: f32 = bounds.size.height.into();
        let x: f32 = local.x.into();
        let y: f32 = local.y.into();
        Some((
            x.clamp(0.0, (width - 1.0).max(0.0)),
            y.clamp(0.0, (height - 1.0).max(0.0)),
        ))
    }

    fn mouse_geometry(&self) -> MouseGeometry {
        let size = self.layout_bounds.map_or_else(
            || Size {
                width: self.cell_size.0 * self.size.0 as f32,
                height: self.cell_size.1 * self.size.1 as f32,
            },
            |bounds| bounds.size,
        );
        let screen_width: f32 = size.width.into();
        let screen_height: f32 = size.height.into();
        let cell_width: f32 = self.cell_size.0.into();
        let cell_height: f32 = self.cell_size.1.into();
        MouseGeometry {
            screen_width: screen_width.round().max(1.0) as u32,
            screen_height: screen_height.round().max(1.0) as u32,
            cell_width: cell_width.round().max(1.0) as u32,
            cell_height: cell_height.round().max(1.0) as u32,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        }
    }

    fn try_open_hyperlink(
        &self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.button != gpui::MouseButton::Left || !hyperlink_modifier(&event.modifiers) {
            return false;
        }
        let Some(point) = self.selection_point(event.position) else {
            return false;
        };
        let Ok(Some(uri)) = self.terminal.hyperlink_uri(point.column, point.row) else {
            return false;
        };
        window.prevent_default();
        cx.stop_propagation();
        if !allowed_hyperlink(&uri) {
            eprintln!("Blocked terminal hyperlink with an unsupported URI scheme");
            return true;
        }
        if let Err(error) = open_hyperlink(&uri) {
            eprintln!("Failed to open terminal hyperlink: {error}");
        }
        true
    }

    fn handle_scrollbar_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scrollbar_dragging = true;
        self.update_scrollbar_from_pointer(event.position, cx);
        window.prevent_default();
        cx.stop_propagation();
    }

    fn handle_scrollbar_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.scrollbar_dragging || !event.dragging() {
            return;
        }
        self.update_scrollbar_from_pointer(event.position, cx);
        cx.stop_propagation();
    }

    fn handle_scrollbar_up(
        &mut self,
        _event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.scrollbar_dragging {
            return;
        }
        self.scrollbar_dragging = false;
        window.prevent_default();
        cx.stop_propagation();
    }

    fn update_scrollbar_from_pointer(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(bounds) = self.layout_bounds else {
            return;
        };
        let Ok(scrollbar) = self.terminal.scrollbar() else {
            return;
        };
        let local_y: f32 = (position.y - bounds.origin.y).into();
        let track_height: f32 = bounds.size.height.into();
        let Some(row) = scrollbar_row_at(scrollbar, track_height, local_y) else {
            return;
        };
        self.terminal.scroll_viewport(ViewportScroll::Row(row));
        cx.notify();
    }

    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        match self.terminal.selected_text() {
            Ok(Some(text)) if !text.is_empty() => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Ok(_) => {}
            Err(error) => eprintln!("Failed to copy terminal selection: {error}"),
        }
    }

    pub(crate) fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if paste::is_safe(text.as_bytes()) {
            self.write_paste(text.as_bytes(), cx);
        } else {
            self.pending_paste = Some(text);
            cx.notify();
        }
    }

    fn confirm_pending_paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.pending_paste.take() else {
            return;
        };
        self.write_paste(text.as_bytes(), cx);
    }

    fn cancel_pending_paste(&mut self, cx: &mut Context<Self>) {
        if self.pending_paste.take().is_some() {
            cx.notify();
        }
    }

    fn write_paste(&mut self, text: &[u8], cx: &mut Context<Self>) {
        match self.terminal.encode_paste(text) {
            Ok(bytes) => {
                self.terminal.scroll_viewport(ViewportScroll::Bottom);
                if let Err(error) = self.terminal.clear_selection() {
                    eprintln!("Failed to clear terminal selection after paste: {error}");
                }
                self.send_pty_command(PtyCommand::Write(bytes));
                self.reset_cursor_blink(cx);
                cx.notify();
            }
            Err(error) => eprintln!("Failed to encode terminal paste: {error}"),
        }
    }

    fn selection_point(&self, position: Point<Pixels>) -> Option<SelectionPoint> {
        terminal_selection_point(position, self.layout_bounds?, self.cell_size, self.size)
    }

    fn selection_geometry(&self) -> SelectionGeometry {
        let cell_width: f32 = self.cell_size.0.into();
        let screen_height: f32 = self
            .layout_bounds
            .map_or(self.cell_size.1 * self.size.1 as f32, |bounds| {
                bounds.size.height
            })
            .into();
        SelectionGeometry {
            columns: u32::from(self.size.0),
            cell_width: cell_width.round().max(1.0) as u32,
            screen_height: screen_height.round().max(1.0) as u32,
        }
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

fn terminal_palette(theme_palette: [gpui::Rgba; 16]) -> Palette {
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

    Palette(palette)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ScrollbarLayout {
    pub top: f32,
    pub height: f32,
}

pub(super) fn scrollbar_layout(scrollbar: Scrollbar, track_height: f32) -> Option<ScrollbarLayout> {
    if scrollbar.total <= scrollbar.len
        || scrollbar.len == 0
        || !track_height.is_finite()
        || track_height <= 0.0
    {
        return None;
    }
    let height = (track_height * scrollbar.len as f32 / scrollbar.total as f32)
        .max(SCROLLBAR_MIN_THUMB_PX)
        .min(track_height);
    let max_top = track_height - height;
    let max_offset = scrollbar.total - scrollbar.len;
    let top = max_top * scrollbar.offset.min(max_offset) as f32 / max_offset as f32;
    Some(ScrollbarLayout { top, height })
}

fn scrollbar_row_at(scrollbar: Scrollbar, track_height: f32, pointer_y: f32) -> Option<usize> {
    let layout = scrollbar_layout(scrollbar, track_height)?;
    let max_top = track_height - layout.height;
    if max_top <= 0.0 {
        return Some(0);
    }
    let thumb_top = (pointer_y - layout.height / 2.0).clamp(0.0, max_top);
    let max_offset = scrollbar.total - scrollbar.len;
    let row = (thumb_top / max_top * max_offset as f32).round() as u64;
    usize::try_from(row).ok()
}

fn ghostty_mouse_button(button: gpui::MouseButton) -> Option<MouseButton> {
    match button {
        gpui::MouseButton::Left => Some(MouseButton::Left),
        gpui::MouseButton::Right => Some(MouseButton::Right),
        gpui::MouseButton::Middle => Some(MouseButton::Middle),
        gpui::MouseButton::Navigate(gpui::NavigationDirection::Back) => Some(MouseButton::Four),
        gpui::MouseButton::Navigate(gpui::NavigationDirection::Forward) => Some(MouseButton::Five),
    }
}

fn hyperlink_modifier(modifiers: &Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.platform
    } else {
        modifiers.control
    }
}

fn allowed_hyperlink(uri: &str) -> bool {
    if uri.is_empty() || uri.chars().any(|character| character.is_control()) {
        return false;
    }
    let Some((scheme, value)) = uri.split_once(':') else {
        return false;
    };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "file"
    ) && value.starts_with("//")
}

fn accepted_terminal_clipboard_text(
    policy: TerminalClipboardPolicy,
    write: ClipboardWrite,
) -> Option<String> {
    if policy != TerminalClipboardPolicy::AllowText || write.location != ClipboardLocation::Standard
    {
        return None;
    }
    if write.contents.is_empty() {
        return Some(String::new());
    }
    let content = write.contents.into_iter().find(|content| {
        content
            .mime
            .split(';')
            .next()
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/plain"))
    })?;
    if content.data.len() > MAX_TERMINAL_CLIPBOARD_BYTES {
        return None;
    }
    String::from_utf8(content.data).ok()
}

#[cfg(target_os = "windows")]
fn open_hyperlink(uri: &str) -> std::io::Result<()> {
    std::process::Command::new("explorer.exe")
        .arg(uri)
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn open_hyperlink(uri: &str) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg(uri)
        .spawn()
        .map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_hyperlink(uri: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(uri)
        .spawn()
        .map(|_| ())
}

#[cfg(not(any(windows, unix)))]
fn open_hyperlink(_uri: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "opening hyperlinks is unsupported on this platform",
    ))
}

fn terminal_selection_point(
    position: Point<Pixels>,
    bounds: Bounds<Pixels>,
    cell_size: (Pixels, Pixels),
    grid_size: (u16, u16),
) -> Option<SelectionPoint> {
    if !bounds.contains(&position) || grid_size.0 == 0 || grid_size.1 == 0 {
        return None;
    }

    let local = position - bounds.origin;
    let surface_x: f32 = local.x.into();
    let surface_y: f32 = local.y.into();
    let cell_width: f32 = cell_size.0.into();
    let cell_height: f32 = cell_size.1.into();
    let column = (surface_x / cell_width).floor() as u16;
    let row = (surface_y / cell_height).floor() as u32;

    Some(SelectionPoint {
        column: column.min(grid_size.0 - 1),
        row: row.min(u32::from(grid_size.1 - 1)),
        surface_x: f64::from(surface_x),
        surface_y: f64::from(surface_y),
    })
}

fn accumulated_scroll_rows(pending: &mut f32, delta: f32, cell_height: f32) -> isize {
    if !delta.is_finite() || !cell_height.is_finite() || cell_height <= 0.0 {
        return 0;
    }

    *pending += delta;
    if !pending.is_finite() {
        *pending = 0.0;
        return 0;
    }
    let rows = (*pending / cell_height).trunc() as isize;
    *pending -= rows as f32 * cell_height;
    rows
}

fn wheel_rows_to_viewport_scroll(wheel_rows: isize) -> ViewportScroll {
    ViewportScroll::Delta(wheel_rows.saturating_neg())
}

fn monotonic_time_ns() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn rectangle_selection(modifiers: &Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.alt
    } else {
        modifiers.alt && (modifiers.control || modifiers.platform)
    }
}

#[cfg(test)]
mod interaction_tests {
    use gpui::{point, size};

    use super::*;

    #[test]
    fn maps_window_position_to_clamped_viewport_cell() {
        let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(100.0), px(60.0)));
        let cell_size = (px(10.0), px(20.0));

        let selected_point =
            terminal_selection_point(point(px(109.0), px(79.0)), bounds, cell_size, (8, 3))
                .unwrap();

        assert_eq!(selected_point.column, 7);
        assert_eq!(selected_point.row, 2);
        assert_eq!(selected_point.surface_x, 99.0);
        assert_eq!(selected_point.surface_y, 59.0);
        assert!(
            terminal_selection_point(point(px(9.0), px(20.0)), bounds, cell_size, (8, 3)).is_none()
        );
    }

    #[test]
    fn accumulates_precise_wheel_motion_by_terminal_row() {
        let mut pending = 0.0;

        assert_eq!(accumulated_scroll_rows(&mut pending, 9.0, 20.0), 0);
        assert_eq!(pending, 9.0);
        assert_eq!(accumulated_scroll_rows(&mut pending, 31.0, 20.0), 2);
        assert_eq!(pending, 0.0);
        assert_eq!(accumulated_scroll_rows(&mut pending, -21.0, 20.0), -1);
        assert_eq!(pending, -1.0);
    }

    #[test]
    fn translates_wheel_direction_to_ghostty_viewport_direction() {
        assert_eq!(wheel_rows_to_viewport_scroll(2), ViewportScroll::Delta(-2));
        assert_eq!(wheel_rows_to_viewport_scroll(-3), ViewportScroll::Delta(3));
        assert_eq!(
            wheel_rows_to_viewport_scroll(isize::MIN),
            ViewportScroll::Delta(isize::MAX)
        );
    }

    #[test]
    fn uses_ghostty_rectangle_selection_modifiers() {
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        assert_eq!(rectangle_selection(&alt), cfg!(target_os = "macos"));

        let control_alt = Modifiers {
            control: true,
            alt: true,
            ..Default::default()
        };
        assert!(rectangle_selection(&control_alt));
    }

    #[test]
    fn lays_out_and_drags_the_scrollbar_in_ghostty_row_space() {
        let scrollbar = Scrollbar {
            total: 100,
            offset: 45,
            len: 10,
        };

        assert_eq!(
            scrollbar_layout(scrollbar, 200.0),
            Some(ScrollbarLayout {
                top: 88.0,
                height: 24.0,
            })
        );
        assert_eq!(scrollbar_row_at(scrollbar, 200.0, 100.0), Some(45));
        assert_eq!(scrollbar_row_at(scrollbar, 200.0, 0.0), Some(0));
        assert_eq!(scrollbar_row_at(scrollbar, 200.0, 200.0), Some(90));
    }

    #[test]
    fn hides_the_scrollbar_without_scrollback() {
        assert_eq!(
            scrollbar_layout(
                Scrollbar {
                    total: 24,
                    offset: 0,
                    len: 24,
                },
                480.0,
            ),
            None
        );
    }

    #[test]
    fn permits_only_supported_hyperlink_schemes() {
        assert!(allowed_hyperlink("https://example.com"));
        assert!(allowed_hyperlink("HTTP://example.com"));
        assert!(allowed_hyperlink("file:///C:/Users/test/readme.txt"));
        assert!(!allowed_hyperlink("mailto:user@example.com"));
        assert!(!allowed_hyperlink("javascript://alert(1)"));
        assert!(!allowed_hyperlink("https://example.com\nunsafe"));
    }

    #[test]
    fn terminal_clipboard_policy_accepts_only_standard_utf8_text() {
        let text_write = ClipboardWrite {
            location: ClipboardLocation::Standard,
            contents: vec![crate::ghostty::ClipboardContent {
                mime: "text/plain;charset=utf-8".to_string(),
                data: b"hello".to_vec(),
            }],
        };
        let primary_write = ClipboardWrite {
            location: ClipboardLocation::Primary,
            ..text_write.clone()
        };
        assert_eq!(
            accepted_terminal_clipboard_text(TerminalClipboardPolicy::Deny, text_write.clone()),
            None
        );
        assert_eq!(
            accepted_terminal_clipboard_text(TerminalClipboardPolicy::AllowText, text_write)
                .as_deref(),
            Some("hello")
        );

        assert_eq!(
            accepted_terminal_clipboard_text(TerminalClipboardPolicy::AllowText, primary_write),
            None
        );
    }
}
