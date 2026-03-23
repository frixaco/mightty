//! Terminal Widget
//!
//! GPUI component that renders terminal content and handles user interaction.
//! Combines ConPtyShell, libghostty Terminal, and InputMapper into a complete
//! terminal widget.

use gpui::{
    div, prelude::*, px, Bounds, Context, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Render, Styled, Window,
};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use crate::ghostty::{Cell, Color, GhosttyTerminalOptions, Terminal};
use crate::input::{Arrow, InputMapper, Key, KeyCode, Modifiers};
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
            cursor_style: CursorStyle::default(),
            cursor_blink: true,
            blink_interval: Duration::from_millis(500),
        }
    }
}

/// Output message from shell reader thread
type OutputData = Vec<u8>;

/// Terminal widget state and rendering
pub struct TerminalWidget {
    /// libghostty terminal emulator
    terminal: Terminal,
    /// Input mapper for key events
    input_mapper: InputMapper,
    /// Configuration
    config: TerminalConfig,
    /// Output data receiver from reader thread
    output_rx: Receiver<OutputData>,
    /// Input data sender to shell (for sending keypresses to shell)
    input_tx: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    /// Resize event sender to shell I/O thread
    resize_tx: Option<Sender<(u16, u16)>>,
    /// Focus handle for tracking focus
    focus_handle: FocusHandle,
    /// Cursor blink state (visible/hidden)
    cursor_blink_phase: bool,
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

        // Clone config for the thread
        let shell_cmd = config.shell.clone();
        let rows = config.initial_rows;
        let cols = config.initial_cols;

        // Start I/O thread that handles both reading and writing (Windows only)
        #[cfg(windows)]
        std::thread::spawn(move || {
            let mut shell = match ConPtyShell::spawn(&shell_cmd, rows, cols) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Failed to spawn shell: {}", e);
                    return;
                }
            };

            let mut buf = [0u8; 4096];
            loop {
                // Check for input (non-blocking)
                match input_rx.try_recv() {
                    Ok(data) => {
                        if shell.write(&data).is_err() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }

                // Check for resize events
                match resize_rx.try_recv() {
                    Ok((rows, cols)) => {
                        if shell.resize(rows, cols).is_err() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }

                // Non-blocking check for output using peek
                match shell.peek() {
                    Ok(true) => match shell.read(&mut buf) {
                        Ok(n) if n > 0 => {
                            if output_tx.send(buf[0..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    },
                    Ok(false) => {
                        // Shell may have exited
                    }
                    Err(_) => break,
                }

                // Standard polling interval (~60Hz)
                std::thread::sleep(Duration::from_millis(16));
            }
        });

        // Start cursor blink timer if enabled
        // TODO: Implement cursor blink timer with correct GPUI types

        let size = (config.initial_cols, config.initial_rows);

        Self {
            terminal,
            input_mapper: InputMapper::new(),
            config,
            output_rx,
            input_tx: Some(input_tx),
            resize_tx: Some(resize_tx),
            focus_handle: cx.focus_handle(),
            cursor_blink_phase: true,
            cursor_pos: (0, 0),
            size,
            cell_size: (px(10.0), px(20.0)), // Monospace cell size (width, height)
            theme: TerminalTheme::default(),
        }
    }

    /// Create a terminal widget with default configuration
    pub fn default(cx: &mut Context<Self>) -> Self {
        Self::new(TerminalConfig::default(), cx)
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
        // Don't process key if we don't have shell input connection
        let input_tx = match &self.input_tx {
            Some(tx) => tx,
            None => return,
        };

        // Convert GPUI key event to our Key type
        let key = self.convert_key_event(event);
        let modifiers = self.convert_modifiers(event);

        // Map to VT sequence
        let vt_bytes = self.input_mapper.map_key_with_modifiers(&key, modifiers);

        // Send VT sequence to shell via ConPTY
        if let Err(e) = input_tx.send(vt_bytes) {
            eprintln!("Failed to send input to shell: {:?}", e);
        }

        cx.notify();
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

    /// Convert GPUI KeyDownEvent to our Key type
    fn convert_key_event(&self, event: &KeyDownEvent) -> Key {
        let keystroke = &event.keystroke;

        // For character keys, use the first character of the key string
        let code = if keystroke.key.len() == 1 {
            // Single character key
            if let Some(c) = keystroke.key.chars().next() {
                KeyCode::Char(c)
            } else {
                KeyCode::Char('?')
            }
        } else {
            // Handle named keys by matching against the string
            let key_str = keystroke.key.as_str();
            match key_str {
                "up" => KeyCode::Arrow(Arrow::Up),
                "down" => KeyCode::Arrow(Arrow::Down),
                "left" => KeyCode::Arrow(Arrow::Left),
                "right" => KeyCode::Arrow(Arrow::Right),
                "home" => KeyCode::Home,
                "end" => KeyCode::End,
                "insert" => KeyCode::Insert,
                "delete" => KeyCode::Delete,
                "pageup" => KeyCode::PageUp,
                "pagedown" => KeyCode::PageDown,
                "escape" => KeyCode::Escape,
                "enter" => KeyCode::Enter,
                "backspace" => KeyCode::Backspace,
                "tab" => KeyCode::Tab,
                "f1" => KeyCode::Function(1),
                "f2" => KeyCode::Function(2),
                "f3" => KeyCode::Function(3),
                "f4" => KeyCode::Function(4),
                "f5" => KeyCode::Function(5),
                "f6" => KeyCode::Function(6),
                "f7" => KeyCode::Function(7),
                "f8" => KeyCode::Function(8),
                "f9" => KeyCode::Function(9),
                "f10" => KeyCode::Function(10),
                "f11" => KeyCode::Function(11),
                "f12" => KeyCode::Function(12),
                _ => KeyCode::Char('?'), // Unknown key
            }
        };

        Key::new(code)
    }

    /// Convert GPUI modifiers to our Modifiers type
    fn convert_modifiers(&self, event: &KeyDownEvent) -> Modifiers {
        let mut modifiers = Modifiers::NONE;
        let keystroke_mods = &event.keystroke.modifiers;

        if keystroke_mods.shift {
            modifiers.insert(Modifiers::SHIFT);
        }
        if keystroke_mods.alt {
            modifiers.insert(Modifiers::ALT);
        }
        if keystroke_mods.control {
            modifiers.insert(Modifiers::CONTROL);
        }
        if keystroke_mods.platform {
            modifiers.insert(Modifiers::SUPER);
        }

        modifiers
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
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(14.0))
                    .font_family("JetBrainsMono NF")
                    .font_weight(font_weight)
                    .when(cell.attrs.italic, |this| this.italic())
                    .when(cell.attrs.underline > 0, |this| this.underline())
                    .when(cell.attrs.strikethrough, |this| this.line_through())
                    .text_color(fg)
                    .child(cell.char.to_string()),
            )
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
            CursorStyle::Line => div()
                .absolute()
                .left(x)
                .top(y)
                .w(px(2.0))
                .h(self.cell_size.1)
                .bg(self.theme.cursor),
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
        // Process any new output
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
        // Cleanup will happen automatically via shell shutdown in reader thread
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
