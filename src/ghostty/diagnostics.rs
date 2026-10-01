//! Bounded observations of the active buffer; no viewport/selection mutation.
use super::{
    Error, Result, Terminal,
    error::from_result,
    ffi,
    render::{CellWidth, Colors},
    style::{RgbColor, Style},
};
use std::mem::size_of;

pub struct DiagnosticCell {
    pub column: u16,
    pub text: String,
    pub width: CellWidth,
    pub foreground: RgbColor,
    pub background: Option<RgbColor>,
    pub style: Style,
}
pub struct DiagnosticRow {
    pub index: usize,
    pub wrapped: bool,
    pub cells: Vec<DiagnosticCell>,
}
pub struct DiagnosticRows {
    pub total: usize,
    pub start: usize,
    pub rows: Vec<DiagnosticRow>,
    pub truncated: bool,
    pub alternate: bool,
}

impl Terminal {
    pub fn viewport_row_wrapped(&self, row: u16) -> Result<bool> {
        let point = ffi::Point {
            tag: ffi::PointTag::VIEWPORT,
            value: ffi::PointValue {
                coordinate: ffi::PointCoordinate {
                    x: 0,
                    y: u32::from(row),
                },
            },
        };
        let mut reference = ffi::GridRef {
            size: size_of::<ffi::GridRef>(),
            ..Default::default()
        };
        from_result(unsafe {
            ffi::ghostty_terminal_grid_ref(self.as_raw(), point, &mut reference)
        })?;
        let mut raw = 0;
        from_result(unsafe { ffi::ghostty_grid_ref_row(&reference, &mut raw) })?;
        let mut wrapped = false;
        from_result(unsafe {
            ffi::ghostty_row_get(
                raw,
                ffi::RowData::WRAP,
                std::ptr::from_mut(&mut wrapped).cast(),
            )
        })?;
        Ok(wrapped)
    }
    pub fn active_buffer_is_alternate(&self) -> Result<bool> {
        let mut screen = ffi::TerminalScreen::PRIMARY;
        from_result(unsafe {
            ffi::ghostty_terminal_get(
                self.as_raw(),
                ffi::TerminalData::ACTIVE_SCREEN,
                std::ptr::from_mut(&mut screen).cast(),
            )
        })?;
        Ok(screen == ffi::TerminalScreen::ALTERNATE)
    }

    pub fn diagnostic_rows(
        &self,
        viewport: bool,
        count: usize,
        byte_limit: usize,
        colors: &Colors,
    ) -> Result<DiagnosticRows> {
        let scrollbar = self.scrollbar()?;
        let mut cols = 0u16;
        from_result(unsafe {
            ffi::ghostty_terminal_get(
                self.as_raw(),
                ffi::TerminalData::COLS,
                std::ptr::from_mut(&mut cols).cast(),
            )
        })?;
        let total = usize::try_from(scrollbar.total).map_err(|_| Error::InvalidValue)?;
        let offset = usize::try_from(scrollbar.offset).map_err(|_| Error::InvalidValue)?;
        let length = usize::try_from(scrollbar.len).map_err(|_| Error::InvalidValue)?;
        let end = if viewport {
            offset.saturating_add(length).min(total)
        } else {
            total
        };
        let start = if viewport {
            offset
        } else {
            end.saturating_sub(count)
        };
        let mut rows = Vec::new();
        let mut bytes = 0usize;
        let mut truncated = false;
        for index in start..end.min(start.saturating_add(count)) {
            let mut cells = Vec::new();
            let mut wrapped = false;
            for column in 0..cols {
                let point = ffi::Point {
                    tag: ffi::PointTag::SCREEN,
                    value: ffi::PointValue {
                        coordinate: ffi::PointCoordinate {
                            x: column,
                            y: index.try_into().map_err(|_| Error::InvalidValue)?,
                        },
                    },
                };
                let mut reference = ffi::GridRef {
                    size: size_of::<ffi::GridRef>(),
                    ..Default::default()
                };
                from_result(unsafe {
                    ffi::ghostty_terminal_grid_ref(self.as_raw(), point, &mut reference)
                })?;
                if column == 0 {
                    let mut raw_row = 0;
                    from_result(unsafe { ffi::ghostty_grid_ref_row(&reference, &mut raw_row) })?;
                    from_result(unsafe {
                        ffi::ghostty_row_get(
                            raw_row,
                            ffi::RowData::WRAP,
                            std::ptr::from_mut(&mut wrapped).cast(),
                        )
                    })?;
                }
                let mut raw_cell = 0;
                from_result(unsafe { ffi::ghostty_grid_ref_cell(&reference, &mut raw_cell) })?;
                let mut width = ffi::CellWide::NARROW;
                from_result(unsafe {
                    ffi::ghostty_cell_get(
                        raw_cell,
                        ffi::CellData::WIDE,
                        std::ptr::from_mut(&mut width).cast(),
                    )
                })?;
                let width = CellWidth::from_raw(width)?;
                let mut graphemes = [0u32; 32];
                let mut length = 0;
                let result = unsafe {
                    ffi::ghostty_grid_ref_graphemes(
                        &reference,
                        graphemes.as_mut_ptr(),
                        graphemes.len(),
                        &mut length,
                    )
                };
                let text = if result == ffi::Result::OUT_OF_SPACE {
                    if length.saturating_mul(4) > byte_limit.saturating_sub(bytes) {
                        truncated = true;
                        break;
                    }
                    let mut data = vec![0u32; length];
                    from_result(unsafe {
                        ffi::ghostty_grid_ref_graphemes(
                            &reference,
                            data.as_mut_ptr(),
                            data.len(),
                            &mut length,
                        )
                    })?;
                    codepoints(&data[..length])?
                } else {
                    from_result(result)?;
                    codepoints(&graphemes[..length])?
                };
                // Account for cell metadata as well as text, bounding JSON allocation/work.
                bytes = bytes.saturating_add(text.len() + 256);
                if bytes > byte_limit {
                    truncated = true;
                    break;
                }
                let mut raw_style = ffi::Style {
                    size: size_of::<ffi::Style>(),
                    ..Default::default()
                };
                from_result(unsafe { ffi::ghostty_grid_ref_style(&reference, &mut raw_style) })?;
                let style = Style::from_raw(raw_style)?;
                let foreground =
                    resolve_color(raw_style.fg_color, colors)?.unwrap_or(colors.foreground);
                let background = resolve_color(raw_style.bg_color, colors)?;
                cells.push(DiagnosticCell {
                    column,
                    text,
                    width,
                    foreground,
                    background,
                    style,
                });
            }
            if truncated {
                break;
            }
            rows.push(DiagnosticRow {
                index,
                wrapped,
                cells,
            });
        }
        Ok(DiagnosticRows {
            total,
            start,
            rows,
            truncated,
            alternate: self.active_buffer_is_alternate()?,
        })
    }
}

fn codepoints(points: &[u32]) -> Result<String> {
    points
        .iter()
        .map(|p| char::from_u32(*p).ok_or(Error::InvalidValue))
        .collect()
}
fn resolve_color(color: ffi::StyleColor, colors: &Colors) -> Result<Option<RgbColor>> {
    Ok(match color.tag {
        ffi::StyleColorTag::NONE => None,
        ffi::StyleColorTag::RGB => Some(unsafe { color.value.rgb }.into()),
        ffi::StyleColorTag::PALETTE => {
            Some(colors.palette[unsafe { color.value.palette } as usize])
        }
        _ => return Err(Error::InvalidValue),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_history_without_mutating_viewport_or_selection() {
        let mut terminal = Terminal::new(super::super::TerminalOptions {
            cols: 10,
            rows: 2,
            max_scrollback: 100,
        })
        .unwrap();
        terminal.vt_write("a  界\r\nb\r\nc\r\nd".as_bytes());
        let before = terminal.scrollbar().unwrap();
        let mut state = super::super::RenderState::new().unwrap();
        let snapshot = state.update(&terminal).unwrap();
        let colors = snapshot.colors().unwrap();
        let rows = terminal
            .diagnostic_rows(false, 100, 65536, &colors)
            .unwrap();
        assert!(rows.rows.len() > 2);
        assert_eq!(rows.rows[0].cells[1].text, " ");
        assert_eq!(terminal.scrollbar().unwrap(), before);
        let limited = terminal.diagnostic_rows(false, 100, 1, &colors).unwrap();
        assert!(limited.truncated);
        assert!(limited.rows.is_empty());
    }
}
