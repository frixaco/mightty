//! Safe semantic prompt and command-output operations.

use std::mem::{MaybeUninit, size_of};

use serde::{Deserialize, Serialize};

use crate::ghostty::error::from_result;
use crate::ghostty::{Error, Result, Terminal, ViewportScroll, ffi};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptDirection {
    Previous,
    Next,
}

impl Terminal {
    pub fn cursor_at_prompt(&self) -> Result<bool> {
        let mut value = false;
        from_result(unsafe {
            ffi::ghostty_terminal_get(
                self.as_raw(),
                ffi::TerminalData::CURSOR_AT_PROMPT,
                std::ptr::from_mut(&mut value).cast(),
            )
        })?;
        Ok(value)
    }
    pub fn has_semantic_prompt(&self) -> Result<bool> {
        let total_rows = self.total_rows()?;
        let first_recent_row = total_rows.saturating_sub(usize::from(self.visible_rows()?));
        for row in (first_recent_row..total_rows).rev() {
            if self.row_is_primary_prompt(row)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn jump_to_prompt(&mut self, direction: PromptDirection) -> Result<bool> {
        let prompts = self.semantic_prompt_rows()?;
        let anchor = self.navigation_anchor()?;
        let target = match direction {
            PromptDirection::Previous => prompts.into_iter().rev().find(|row| *row < anchor),
            PromptDirection::Next => prompts.into_iter().find(|row| *row > anchor),
        };
        let Some(target) = target else {
            return Ok(false);
        };
        self.scroll_viewport(ViewportScroll::Row(target));
        Ok(true)
    }

    pub fn select_command_output(&mut self) -> Result<bool> {
        let Some(selection) = self.preceding_command_output_selection()? else {
            return Ok(false);
        };
        let mut start = ffi::PointCoordinate::default();
        let result = unsafe {
            ffi::ghostty_terminal_point_from_grid_ref(
                self.as_raw(),
                &raw const selection.start,
                ffi::PointTag::SCREEN,
                &raw mut start,
            )
        };
        from_result(result)?;
        let result = unsafe {
            ffi::ghostty_terminal_set(
                self.as_raw(),
                ffi::TerminalOption::SELECTION,
                std::ptr::from_ref(&selection).cast(),
            )
        };
        from_result(result)?;
        self.scroll_viewport(ViewportScroll::Row(start.y as usize));
        Ok(true)
    }

    pub fn command_output_text(&self) -> Result<Option<String>> {
        let Some(selection) = self.preceding_command_output_selection()? else {
            return Ok(None);
        };
        self.format_selection_text(Some(&selection))
    }

    fn preceding_command_output_selection(&self) -> Result<Option<ffi::Selection>> {
        let cursor_row = self
            .scrollback_rows()?
            .checked_add(usize::from(self.cursor_y()?))
            .ok_or(Error::InvalidValue)?;
        let prompts = self.semantic_prompt_rows()?;
        let Some(current_prompt_index) = prompts.iter().rposition(|row| *row <= cursor_row) else {
            return Ok(None);
        };
        let Some(previous_prompt) = current_prompt_index
            .checked_sub(1)
            .map(|index| prompts[index])
        else {
            return Ok(None);
        };
        let current_prompt = prompts[current_prompt_index];
        let cols = self.cols()?;
        for row in (previous_prompt.saturating_add(1)..current_prompt).rev() {
            for column in 0..cols {
                let grid_ref = self.grid_ref_at_screen_point(column, row)?;
                let mut cell = 0;
                let result =
                    unsafe { ffi::ghostty_grid_ref_cell(&raw const grid_ref, &raw mut cell) };
                from_result(result)?;
                let has_text = unsafe { cell_value::<bool>(cell, ffi::CellData::HAS_TEXT)? };
                let semantic = unsafe {
                    cell_value::<ffi::CellSemanticContent::Type>(
                        cell,
                        ffi::CellData::SEMANTIC_CONTENT,
                    )?
                };
                if has_text && semantic == ffi::CellSemanticContent::OUTPUT {
                    return self.output_selection_at(grid_ref);
                }
            }
        }
        Ok(None)
    }

    fn semantic_prompt_rows(&self) -> Result<Vec<usize>> {
        let total_rows = self.total_rows()?;
        let mut prompts = Vec::new();
        for row in 0..total_rows {
            if self.row_is_primary_prompt(row)? {
                prompts.push(row);
            }
        }
        Ok(prompts)
    }

    fn navigation_anchor(&self) -> Result<usize> {
        if self.viewport_active()? {
            return self
                .scrollback_rows()?
                .checked_add(usize::from(self.cursor_y()?))
                .ok_or(Error::InvalidValue);
        }
        usize::try_from(self.scrollbar()?.offset).map_err(|_| Error::InvalidValue)
    }

    fn total_rows(&self) -> Result<usize> {
        unsafe { terminal_value(self, ffi::TerminalData::TOTAL_ROWS) }
    }

    fn scrollback_rows(&self) -> Result<usize> {
        unsafe { terminal_value(self, ffi::TerminalData::SCROLLBACK_ROWS) }
    }

    fn cursor_y(&self) -> Result<u16> {
        unsafe { terminal_value(self, ffi::TerminalData::CURSOR_Y) }
    }

    fn cols(&self) -> Result<u16> {
        unsafe { terminal_value(self, ffi::TerminalData::COLS) }
    }

    fn visible_rows(&self) -> Result<u16> {
        unsafe { terminal_value(self, ffi::TerminalData::ROWS) }
    }

    fn viewport_active(&self) -> Result<bool> {
        unsafe { terminal_value(self, ffi::TerminalData::VIEWPORT_ACTIVE) }
    }

    fn grid_ref_at_screen_point(&self, column: u16, row: usize) -> Result<ffi::GridRef> {
        let y = u32::try_from(row).map_err(|_| Error::InvalidValue)?;
        let point = ffi::Point {
            tag: ffi::PointTag::SCREEN,
            value: ffi::PointValue {
                coordinate: ffi::PointCoordinate { x: column, y },
            },
        };
        let mut grid_ref = ffi::GridRef {
            size: size_of::<ffi::GridRef>(),
            ..Default::default()
        };
        let result =
            unsafe { ffi::ghostty_terminal_grid_ref(self.as_raw(), point, &raw mut grid_ref) };
        from_result(result)?;
        Ok(grid_ref)
    }

    fn row_is_primary_prompt(&self, row: usize) -> Result<bool> {
        let grid_ref = self.grid_ref_at_screen_point(0, row)?;
        let mut raw_row = 0;
        let result = unsafe { ffi::ghostty_grid_ref_row(&raw const grid_ref, &raw mut raw_row) };
        from_result(result)?;
        let semantic = unsafe {
            row_value::<ffi::RowSemanticPrompt::Type>(raw_row, ffi::RowData::SEMANTIC_PROMPT)?
        };
        match semantic {
            ffi::RowSemanticPrompt::PROMPT => Ok(true),
            ffi::RowSemanticPrompt::NONE | ffi::RowSemanticPrompt::PROMPT_CONTINUATION => Ok(false),
            _ => Err(Error::InvalidValue),
        }
    }

    fn output_selection_at(&self, grid_ref: ffi::GridRef) -> Result<Option<ffi::Selection>> {
        let mut selection = ffi::Selection {
            size: size_of::<ffi::Selection>(),
            ..Default::default()
        };
        let result = unsafe {
            ffi::ghostty_terminal_select_output(self.as_raw(), grid_ref, &raw mut selection)
        };
        match result {
            ffi::Result::NO_VALUE => return Ok(None),
            ffi::Result::SUCCESS => {}
            other => {
                from_result(other)?;
                unreachable!("successful and missing semantic output handled above")
            }
        }
        Ok(Some(selection))
    }
}

/// `T` must be the output type documented for `data`.
unsafe fn terminal_value<T>(terminal: &Terminal, data: ffi::TerminalData::Type) -> Result<T> {
    let mut value = MaybeUninit::<T>::uninit();
    let result =
        unsafe { ffi::ghostty_terminal_get(terminal.as_raw(), data, value.as_mut_ptr().cast()) };
    from_result(result)?;
    Ok(unsafe { value.assume_init() })
}

/// `T` must be the output type documented for `data`.
unsafe fn row_value<T>(row: ffi::Row, data: ffi::RowData::Type) -> Result<T> {
    let mut value = MaybeUninit::<T>::uninit();
    let result = unsafe { ffi::ghostty_row_get(row, data, value.as_mut_ptr().cast()) };
    from_result(result)?;
    Ok(unsafe { value.assume_init() })
}

/// `T` must be the output type documented for `data`.
unsafe fn cell_value<T>(cell: ffi::Cell, data: ffi::CellData::Type) -> Result<T> {
    let mut value = MaybeUninit::<T>::uninit();
    let result = unsafe { ffi::ghostty_cell_get(cell, data, value.as_mut_ptr().cast()) };
    from_result(result)?;
    Ok(unsafe { value.assume_init() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ghostty::TerminalOptions;

    fn terminal_with_commands() -> Terminal {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 40,
            rows: 2,
            max_scrollback: 100,
        })
        .unwrap();
        terminal.vt_write(
            b"\x1b]133;A\x07PS> \x1b]133;B\x07echo one\r\n\
              \x1b]133;C\x07one\r\n\x1b]133;D;0\x07\
              \x1b]133;A\x07PS> \x1b]133;B\x07echo two\r\n\
              \x1b]133;C\x07two\r\n\x1b]133;D;0\x07\
              \x1b]133;A\x07PS> \x1b]133;B\x07",
        );
        terminal
    }

    #[test]
    fn finds_and_navigates_semantic_prompts() {
        let mut terminal = terminal_with_commands();
        assert!(terminal.has_semantic_prompt().unwrap());

        let at_bottom = terminal.scrollbar().unwrap();
        assert!(terminal.jump_to_prompt(PromptDirection::Previous).unwrap());
        assert!(terminal.scrollbar().unwrap().offset < at_bottom.offset);
        assert!(terminal.jump_to_prompt(PromptDirection::Next).unwrap());
    }

    #[test]
    fn selects_the_preceding_command_output() {
        let mut terminal = terminal_with_commands();

        assert!(terminal.select_command_output().unwrap());
        assert_eq!(terminal.selected_text().unwrap().as_deref(), Some("two"));
    }

    #[test]
    fn formats_command_output_without_changing_the_selection() {
        let terminal = terminal_with_commands();

        assert_eq!(
            terminal.command_output_text().unwrap().as_deref(),
            Some("two")
        );
        assert_eq!(terminal.selected_text().unwrap(), None);
    }

    #[test]
    fn does_not_skip_an_empty_command_to_select_older_output() {
        let mut terminal = terminal_with_commands();
        terminal.vt_write(
            b"true\r\n\x1b]133;C\x07\x1b]133;D;0\x07\
              \x1b]133;A\x07PS> \x1b]133;B\x07",
        );

        assert!(!terminal.select_command_output().unwrap());
        assert_eq!(terminal.selected_text().unwrap(), None);
    }
}
