# Terminal Widget

## Purpose
GPUI component that renders terminal content and handles user interaction.

## Component Structure

```rust
pub struct TerminalWidget {
    shell: ConPtyShell,
    terminal: Terminal,
    screen: ScreenBuffer,
    input_mapper: InputMapper,
    
    // Configuration
    font: Font,
    font_size: Pixels,
    theme: Theme,
    
    // State
    cursor_visible: bool,
    cursor_blink_phase: bool,
    focused: bool,
}

impl Render for TerminalWidget {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement;
}
```

## Lifecycle

### Creation
1. Initialize libghostty Terminal
2. Spawn ConPTY shell
3. Start output reader thread
4. Begin cursor blink timer

### Update Loop
1. Output thread pushes bytes to channel
2. Main thread receives bytes, feeds to libghostty
3. Screen buffer refreshed from libghostty state
4. GPUI notifies to re-render

### Event Handling
- **KeyDown**: Map to VT, send to shell
- **FocusIn/Out**: Show/hide cursor, bell
- **Resize**: Update terminal size, notify shell

## Rendering

### Layout Calculation
```rust
fn calculate_dimensions(&self, bounds: Bounds<Pixels>) -> (u16, u16) {
    let char_width = self.font.advance('M');
    let char_height = self.font.line_height();
    
    let cols = (bounds.width / char_width) as u16;
    let rows = (bounds.height / char_height) as u16;
    
    (rows, cols)
}
```

### Cell Rendering
```rust
fn render_cell(&self, cell: &Cell, row: u16, col: u16) -> impl IntoElement {
    div()
        .position(Position::Absolute)
        .left(px(col as f32 * self.cell_width))
        .top(px(row as f32 * self.cell_height))
        .size(px(self.cell_width), px(self.cell_height))
        .bg(cell.bg.to_gpui_color())
        .child(
            text(cell.char.to_string())
                .color(cell.fg.to_gpui_color())
                .font_weight(if cell.attrs.bold { Bold } else { Normal })
                .italic(cell.attrs.italic)
        )
}
```

### Cursor Rendering
```rust
fn render_cursor(&self, pos: (u16, u16)) -> impl IntoElement {
    let style = if self.cursor_blink_phase || !self.focused {
        CursorStyle::Block  // Solid
    } else {
        CursorStyle::Hidden
    };
    
    match style {
        CursorStyle::Block => div()
            .absolute()
            .position(cursor_pos)
            .size(cell_size)
            .bg(cursor_color)
            .text_color(inverted_fg),
        CursorStyle::Line => div()
            .absolute()
            .position(cursor_pos)
            .w(px(2.0))
            .h(cell_height)
            .bg(cursor_color),
    }
}
```

## Focus Management

- Click to focus widget
- Show cursor only when focused
- Optional: hide cursor when typing (decset 25)

## Configuration

```rust
pub struct TerminalConfig {
    pub shell: String,           // "cmd", "pwsh", "wsl"
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,        // Multiplier
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    pub theme: Theme,
    pub scrollback: usize,
}
```

## Thread Safety

```rust
// Output reader thread
thread::spawn(move || {
    let mut buf = [0u8; 4096];
    loop {
        match shell.read(&mut buf) {
            Ok(n) if n > 0 => {
                output_tx.send(buf[0..n].to_vec()).ok();
            }
            _ => break,
        }
    }
});

// Main thread handling
cx.on_next_frame(move |this, cx| {
    while let Ok(data) = output_rx.try_recv() {
        this.terminal.write(&data);
        cx.notify();
    }
});
```

## Future Features

- Text selection (click-drag)
- Copy/paste
- Search
- Split panes (v/h)
- Tab management
