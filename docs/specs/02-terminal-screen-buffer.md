# Terminal Screen Buffer

## Purpose
Bridge between libghostty-vt's internal state and renderable cell data.

## Data Structures

```rust
pub struct Cell {
    pub char: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attributes,
}

pub struct Attributes {
    pub bold: bool,
    pub italic: bool,
    pub underline: u8,  // 0=none, 1=single, 2=double
    pub strikethrough: bool,
    pub inverse: bool,
    pub blink: bool,
}

pub enum Color {
    Default,
    Palette(u8),         // 0-255
    Rgb(u8, u8, u8),
}

pub struct ScreenBuffer {
    rows: u16,
    cols: u16,
    cells: Vec<Vec<Cell>>,
    cursor: (u16, u16),
}
```

## Interface

```rust
impl Terminal {
    /// Read cells from libghostty screen state
    pub fn read_screen(&self) -> ScreenBuffer;
    
    /// Get single cell at position
    pub fn cell_at(&self, row: u16, col: u16) -> Option<Cell>;
    
    /// Get cursor position in screen coordinates
    pub fn cursor_screen_pos(&self) -> (u16, u16);
    
    /// Check if cell has changed since last read (for damage tracking)
    pub fn cell_modified(&self, row: u16, col: u16) -> bool;
}
```

## FFI Requirements

Add bindings for:
```c
// Get cell data at row, col
ghostty_terminal_get_cell(terminal, row, col, cell_out);

// Get entire row
ghostty_terminal_get_row(terminal, row, cells_out, max_cells);

// Query cell attributes
ghostty_terminal_cell_fg_color(terminal, row, col);
ghostty_terminal_cell_bg_color(terminal, row, col);
ghostty_terminal_cell_attrs(terminal, row, col);
```

## Color Mapping

| Source | Mapping |
|--------|---------|
| ANSI 0-7 | Theme colors (configurable) |
| ANSI 8-15 | Bright variants |
| 256 palette | xterm palette table |
| RGB | Direct values |

## Performance

- Cache last screen state
- Only re-read modified cells
- Batch row reads vs individual cells
- Pre-allocate cell buffers

## Wide Characters

- CJK characters: width 2, occupies 2 columns
- Surrogate pairs: single char, width based on unicode width
- Combining marks: attached to previous cell
