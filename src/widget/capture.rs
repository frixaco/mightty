use crate::feedback::{
    self, CaptureCell, CaptureColors, CaptureCursor, CaptureRow, FontCapture, GridSize, RgbHex,
    SizePx, TerminalCapture,
};
use crate::ghostty::{
    render::CellWidth,
    style::{RgbColor, Underline},
};

use super::render::{CellWidthExt, RenderCellExt};
use super::{TERMINAL_FONT_FAMILY, TERMINAL_FONT_SIZE_PX, TerminalWidget};

impl TerminalWidget {
    pub(super) fn build_feedback_capture(&mut self) -> crate::ghostty::Result<TerminalCapture> {
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
        _ => "unknown",
    }
}
