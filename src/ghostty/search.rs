use std::ptr::NonNull;

use crate::ghostty::error::{from_result, from_result_with_len};
use crate::ghostty::{Error, Result, Terminal, ffi};

/// Progress from one bounded search step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchProgress {
    Pending,
    Complete,
}

/// Direction through search results, from the newest result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchDirection {
    Next,
    Previous,
}

/// A point in the full active screen, including scrollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchPoint {
    pub column: u16,
    pub row: u32,
}

/// An inclusive search range in full-screen coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchRange {
    pub start: SearchPoint,
    pub end: SearchPoint,
}

pub(crate) struct SearchState {
    raw: NonNull<ffi::MighttyGhosttySearchImpl>,
}

impl SearchState {
    fn new(terminal: ffi::Terminal, query: &str) -> Result<Self> {
        let mut raw = std::ptr::null_mut();
        let result = unsafe {
            ffi::mightty_ghostty_search_new(terminal, query.as_ptr(), query.len(), &raw mut raw)
        };
        from_result(result)?;
        Ok(Self {
            raw: NonNull::new(raw).ok_or(Error::InvalidValue)?,
        })
    }

    fn as_raw(&self) -> ffi::MighttyGhosttySearch {
        self.raw.as_ptr()
    }
}

impl Drop for SearchState {
    fn drop(&mut self) {
        unsafe {
            ffi::mightty_ghostty_search_free(self.as_raw());
        }
    }
}

impl Terminal {
    /// Start a search. An empty query stops the current search.
    pub fn start_search(&mut self, query: &str) -> Result<()> {
        if query.is_empty() {
            self.stop_search();
            return Ok(());
        }

        let search = SearchState::new(self.as_raw(), query)?;
        self.search = Some(search);
        Ok(())
    }

    /// Stop the current search and release its tracked ranges.
    pub fn stop_search(&mut self) {
        self.search = None;
    }

    /// Advance the current search by one bounded engine step.
    pub fn search_step(&mut self) -> Result<SearchProgress> {
        let search = self.search.as_mut().ok_or(Error::InvalidValue)?;
        let mut progress = ffi::MighttyGhosttySearchStep::PENDING;
        let result =
            unsafe { ffi::mightty_ghostty_search_step(search.as_raw(), &raw mut progress) };
        from_result(result)?;
        match progress {
            ffi::MighttyGhosttySearchStep::PENDING => Ok(SearchProgress::Pending),
            ffi::MighttyGhosttySearchStep::COMPLETE => Ok(SearchProgress::Complete),
            _ => Err(Error::InvalidValue),
        }
    }

    /// Get all results found by the current search.
    pub fn search_ranges(&mut self) -> Result<Vec<SearchRange>> {
        let search = self.search.as_mut().ok_or(Error::InvalidValue)?;
        let mut required = 0;
        let result = unsafe {
            ffi::mightty_ghostty_search_ranges(
                search.as_raw(),
                std::ptr::null_mut(),
                0,
                &raw mut required,
            )
        };
        match result {
            ffi::Result::SUCCESS if required == 0 => return Ok(Vec::new()),
            ffi::Result::OUT_OF_SPACE => {}
            other => {
                from_result_with_len(other, required)?;
                return Err(Error::InvalidValue);
            }
        }

        let mut ranges = vec![ffi::MighttyGhosttySearchRange::default(); required];
        let mut written = 0;
        let result = unsafe {
            ffi::mightty_ghostty_search_ranges(
                search.as_raw(),
                ranges.as_mut_ptr(),
                ranges.len(),
                &raw mut written,
            )
        };
        let written = from_result_with_len(result, written)?;
        if written > ranges.len() {
            return Err(Error::InvalidValue);
        }
        ranges.truncate(written);
        Ok(ranges.into_iter().map(SearchRange::from).collect())
    }

    /// Select one result and return its current tracked range.
    pub fn search_select(&mut self, direction: SearchDirection) -> Result<Option<SearchRange>> {
        let search = self.search.as_mut().ok_or(Error::InvalidValue)?;
        let direction = match direction {
            SearchDirection::Next => ffi::MighttyGhosttySearchDirection::NEXT,
            SearchDirection::Previous => ffi::MighttyGhosttySearchDirection::PREVIOUS,
        };
        let mut range = ffi::MighttyGhosttySearchRange::default();
        let result = unsafe {
            ffi::mightty_ghostty_search_select(search.as_raw(), direction, &raw mut range)
        };
        match result {
            ffi::Result::SUCCESS => Ok(Some(range.into())),
            ffi::Result::NO_VALUE => Ok(None),
            other => {
                from_result(other)?;
                Err(Error::InvalidValue)
            }
        }
    }
}

impl From<ffi::MighttyGhosttySearchRange> for SearchRange {
    fn from(value: ffi::MighttyGhosttySearchRange) -> Self {
        Self {
            start: SearchPoint {
                column: value.start_x,
                row: value.start_y,
            },
            end: SearchPoint {
                column: value.end_x,
                row: value.end_y,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ghostty::TerminalOptions;

    #[test]
    fn searches_the_full_scrollback() {
        let mut terminal = search_terminal();
        terminal.vt_write(b"needle one\r\nskip\r\nneedle two\r\nskip\r\nneedle three\r\nskip");
        assert!(terminal.scrollbar().unwrap().total > terminal.scrollbar().unwrap().len);

        terminal.start_search("needle").unwrap();
        finish_search(&mut terminal);
        let ranges = terminal.search_ranges().unwrap();

        assert_eq!(ranges.len(), 3);
        assert!(ranges.iter().any(|range| range.start.row < 2));
        assert!(ranges.iter().any(|range| range.start.row >= 2));
    }

    #[test]
    fn refreshes_results_after_continued_output() {
        let mut terminal = search_terminal();
        terminal.vt_write(b"needle one\r\nskip\r\nneedle two");
        terminal.start_search("needle").unwrap();
        finish_search(&mut terminal);
        let initial_count = terminal.search_ranges().unwrap().len();

        terminal.vt_write(b"\r\nneedle three");
        finish_search(&mut terminal);

        assert_eq!(terminal.search_ranges().unwrap().len(), initial_count + 1);
    }

    #[test]
    fn navigates_results_in_both_directions() {
        let mut terminal = search_terminal();
        terminal.vt_write(b"needle one\r\nskip\r\nneedle two");
        terminal.start_search("needle").unwrap();
        finish_search(&mut terminal);

        let newest = terminal
            .search_select(SearchDirection::Next)
            .unwrap()
            .unwrap();
        let older = terminal
            .search_select(SearchDirection::Next)
            .unwrap()
            .unwrap();
        let newest_again = terminal
            .search_select(SearchDirection::Previous)
            .unwrap()
            .unwrap();

        assert!(older.start.row < newest.start.row);
        assert_eq!(newest_again, newest);
    }

    fn search_terminal() -> Terminal {
        Terminal::new(TerminalOptions {
            cols: 16,
            rows: 2,
            max_scrollback: 100,
        })
        .unwrap()
    }

    fn finish_search(terminal: &mut Terminal) {
        for _ in 0..1024 {
            if terminal.search_step().unwrap() == SearchProgress::Complete {
                return;
            }
        }
        panic!("search did not finish");
    }
}
