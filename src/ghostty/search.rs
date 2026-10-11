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

/// Search results as of the last feed, sized for drawing rather than
/// listing every match in the scrollback.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResults {
    /// Matches on the active screen, including scrollback.
    pub total: usize,
    /// The selected match and its index, where 0 is the newest match.
    pub selected: Option<(usize, SearchRange)>,
    /// Matches on the pages covering the viewport; may include a few rows
    /// just outside it.
    pub visible: Vec<SearchRange>,
}

/// A Ghostty search bound to its terminal. Ghostty keeps its results in sync
/// with the live screens; mightty never scrolls the viewport through it.
pub(crate) struct SearchState {
    raw: NonNull<ffi::SearchImpl>,
}

impl SearchState {
    fn new(terminal: ffi::Terminal, query: &str) -> Result<Self> {
        let mut raw = std::ptr::null_mut();
        from_result(unsafe { ffi::ghostty_search_new(std::ptr::null(), &raw mut raw, terminal) })?;
        // Owned from here so every failure below frees the search.
        let search = Self {
            raw: NonNull::new(raw).ok_or(Error::InvalidValue)?,
        };
        let needle = ffi::String {
            ptr: query.as_ptr(),
            len: query.len(),
        };
        from_result(unsafe {
            ffi::ghostty_search_set(
                search.as_raw(),
                ffi::SearchOption::GHOSTTY_SEARCH_OPT_NEEDLE,
                std::ptr::from_ref(&needle).cast(),
            )
        })?;
        let scroll = ffi::SearchScroll::NONE;
        from_result(unsafe {
            ffi::ghostty_search_set(
                search.as_raw(),
                ffi::SearchOption::GHOSTTY_SEARCH_OPT_SELECT_SCROLL,
                std::ptr::from_ref(&scroll).cast(),
            )
        })?;
        Ok(search)
    }

    fn as_raw(&self) -> ffi::Search {
        self.raw.as_ptr()
    }

    /// One bounded step: tick copied data, then feed when the search has
    /// caught up or `refresh` reports terminal changes it has not seen.
    fn step(&mut self, refresh: bool) -> Result<SearchProgress> {
        let mut status = ffi::SearchStatus::RUNNING;
        from_result(unsafe { ffi::ghostty_search_tick(self.as_raw(), &raw mut status) })?;
        if refresh || status != ffi::SearchStatus::RUNNING {
            // Feeding picks up output, viewport movement and pruned history.
            from_result(unsafe { ffi::ghostty_search_feed(self.as_raw()) })?;
            status = unsafe { self.get(ffi::SearchData::STATUS) }?;
        }
        Ok(if status == ffi::SearchStatus::COMPLETE {
            SearchProgress::Complete
        } else {
            SearchProgress::Pending
        })
    }

    fn total_matches(&self) -> Result<usize> {
        unsafe { self.get(ffi::SearchData::TOTAL_MATCHES) }
    }

    /// Counts and the matches needed to draw the viewport as of the last feed.
    fn results(&self, terminal: ffi::Terminal) -> Result<SearchResults> {
        let visible = self
            .viewport_selections()?
            .iter()
            .map(|selection| selection_range(terminal, selection))
            .collect::<Result<_>>()?;
        Ok(SearchResults {
            total: self.total_matches()?,
            selected: self.selected(terminal)?,
            visible,
        })
    }

    fn viewport_selections(&self) -> Result<Vec<ffi::Selection>> {
        let mut buffer = ffi::SelectionBuffer::default();
        match unsafe { self.get_raw(ffi::SearchData::VIEWPORT_MATCHES, &mut buffer) } {
            ffi::Result::SUCCESS => return Ok(Vec::new()),
            ffi::Result::OUT_OF_SPACE => {}
            other => {
                from_result(other)?;
                return Err(Error::InvalidValue);
            }
        }
        let mut selections = vec![ffi::Selection::default(); buffer.len];
        buffer.ptr = selections.as_mut_ptr();
        buffer.cap = selections.len();
        let result = unsafe { self.get_raw(ffi::SearchData::VIEWPORT_MATCHES, &mut buffer) };
        let written = from_result_with_len(result, buffer.len)?;
        if written > selections.len() {
            return Err(Error::InvalidValue);
        }
        selections.truncate(written);
        Ok(selections)
    }

    /// The selected match and its index, newest first.
    fn selected(&self, terminal: ffi::Terminal) -> Result<Option<(usize, SearchRange)>> {
        let mut index = 0_usize;
        match unsafe { self.get_raw(ffi::SearchData::SELECTED_INDEX, &mut index) } {
            ffi::Result::NO_VALUE => return Ok(None),
            result => from_result(result)?,
        }
        let mut selection = ffi::Selection::default();
        from_result(unsafe { self.get_raw(ffi::SearchData::SELECTED_MATCH, &mut selection) })?;
        Ok(Some((index, selection_range(terminal, &selection)?)))
    }

    fn select(
        &mut self,
        terminal: ffi::Terminal,
        direction: SearchDirection,
    ) -> Result<Option<(usize, SearchRange)>> {
        let option = match direction {
            SearchDirection::Next => ffi::SearchOption::GHOSTTY_SEARCH_OPT_SELECT_NEXT,
            SearchDirection::Previous => ffi::SearchOption::GHOSTTY_SEARCH_OPT_SELECT_PREV,
        };
        match unsafe { ffi::ghostty_search_set(self.as_raw(), option, std::ptr::null()) } {
            ffi::Result::NO_VALUE => Ok(None),
            result => {
                from_result(result)?;
                self.selected(terminal)
            }
        }
    }

    /// `T` must be the output type documented for `data`.
    unsafe fn get<T: Default>(&self, data: ffi::SearchData::Type) -> Result<T> {
        let mut value = T::default();
        from_result(unsafe { self.get_raw(data, &mut value) })?;
        Ok(value)
    }

    /// `T` must be the output type documented for `data`.
    unsafe fn get_raw<T>(&self, data: ffi::SearchData::Type, value: &mut T) -> ffi::Result::Type {
        unsafe { ffi::ghostty_search_get(self.as_raw(), data, std::ptr::from_mut(value).cast()) }
    }
}

impl Drop for SearchState {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_search_free(self.as_raw());
        }
    }
}

/// Convert an untracked match snapshot before the terminal changes again.
fn selection_range(terminal: ffi::Terminal, selection: &ffi::Selection) -> Result<SearchRange> {
    let point = |reference: &ffi::GridRef| {
        let mut coordinate = ffi::PointCoordinate::default();
        from_result(unsafe {
            ffi::ghostty_terminal_point_from_grid_ref(
                terminal,
                reference,
                ffi::PointTag::SCREEN,
                &raw mut coordinate,
            )
        })?;
        Ok(SearchPoint {
            column: coordinate.x,
            row: coordinate.y,
        })
    };
    Ok(SearchRange {
        start: point(&selection.start)?,
        end: point(&selection.end)?,
    })
}

/// A control text wait: its search and the buffer it must stay on.
pub(crate) struct ControlSearch {
    search: SearchState,
    alternate: bool,
    resets: u64,
}

impl Terminal {
    pub fn register_search(&mut self, query: &str) -> Result<u64> {
        if self.control_searches.len() >= 32 || query.is_empty() || query.len() > 4096 {
            return Err(Error::InvalidValue);
        }
        let search = ControlSearch {
            search: SearchState::new(self.as_raw(), query)?,
            alternate: self.active_buffer_is_alternate()?,
            resets: self.reset_generation(),
        };
        let id = self.next_control_search;
        self.next_control_search += 1;
        self.control_searches.insert(id, search);
        Ok(id)
    }
    /// Returns (matched, buffer changed) without using interactive search state.
    pub fn probe_search(&mut self, id: u64) -> Result<(bool, bool)> {
        let alternate = self.active_buffer_is_alternate()?;
        let resets = self.reset_generation();
        let control = self
            .control_searches
            .get_mut(&id)
            .ok_or(Error::InvalidValue)?;
        // A wait belongs to one buffer; a switch or full reset ends it.
        if control.alternate != alternate || control.resets != resets {
            return Ok((false, true));
        }
        control.search.step(true)?;
        Ok((control.search.total_matches()? > 0, false))
    }
    pub fn release_search(&mut self, id: u64) {
        self.control_searches.remove(&id);
    }
    /// Start a search. An empty query stops the current search.
    pub fn start_search(&mut self, query: &str) -> Result<()> {
        if query.is_empty() {
            self.stop_search();
            return Ok(());
        }

        // Release the old search's tracked state before creating the next.
        self.search = None;
        self.search = Some(SearchState::new(self.as_raw(), query)?);
        Ok(())
    }

    /// Stop the current search and release its tracked ranges.
    pub fn stop_search(&mut self) {
        self.search = None;
    }

    /// Advance the current search by one bounded engine step. Pass `refresh`
    /// after output, scrolling or resizing so results follow the terminal.
    pub fn search_step(&mut self, refresh: bool) -> Result<SearchProgress> {
        self.search
            .as_mut()
            .ok_or(Error::InvalidValue)?
            .step(refresh)
    }

    /// Results of the current search as of its last step.
    pub fn search_results(&mut self) -> Result<SearchResults> {
        let terminal = self.as_raw();
        self.search
            .as_ref()
            .ok_or(Error::InvalidValue)?
            .results(terminal)
    }

    /// Select one result and return its index and current range. Selection
    /// wraps around past the oldest and newest results.
    pub fn search_select(
        &mut self,
        direction: SearchDirection,
    ) -> Result<Option<(usize, SearchRange)>> {
        let terminal = self.as_raw();
        self.search
            .as_mut()
            .ok_or(Error::InvalidValue)?
            .select(terminal, direction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ghostty::{TerminalOptions, ViewportScroll};

    #[test]
    fn diagnostic_search_is_independent_and_detects_buffer_switch() {
        let mut terminal = search_terminal();
        terminal.vt_write(b"needle");
        terminal.start_search("other").unwrap();
        let id = terminal.register_search("needle").unwrap();
        let before = terminal.scrollbar().unwrap();
        let mut matched = false;
        for _ in 0..128 {
            let result = terminal.probe_search(id).unwrap();
            assert!(!result.1);
            matched |= result.0;
            if matched {
                break;
            }
        }
        assert!(matched);
        assert_eq!(terminal.scrollbar().unwrap().offset, before.offset);
        finish_search(&mut terminal);
        assert_eq!(terminal.search_results().unwrap().total, 0);
        terminal.vt_write(b"\x1b[?1049h");
        assert!(terminal.probe_search(id).unwrap().1);
        terminal.release_search(id);
    }

    #[test]
    fn diagnostic_search_detects_full_reset() {
        let mut terminal = search_terminal();
        let id = terminal.register_search("needle").unwrap();
        assert_eq!(terminal.probe_search(id).unwrap(), (false, false));
        // A reset on the same (primary) buffer still invalidates the wait.
        terminal.vt_write(b"\x1bcneedle");
        assert_eq!(terminal.probe_search(id).unwrap(), (false, true));
    }

    #[test]
    fn searches_the_full_scrollback() {
        let mut terminal = search_terminal();
        terminal.vt_write(b"needle one\r\nskip\r\nneedle two\r\nskip\r\nneedle three\r\nskip");
        assert!(terminal.scrollbar().unwrap().total > terminal.scrollbar().unwrap().len);

        terminal.start_search("needle").unwrap();
        finish_search(&mut terminal);
        assert_eq!(terminal.search_results().unwrap().total, 3);
        let ranges = (0..3)
            .map(|_| {
                terminal
                    .search_select(SearchDirection::Next)
                    .unwrap()
                    .unwrap()
                    .1
            })
            .collect::<Vec<_>>();

        assert!(ranges.iter().any(|range| range.start.row < 2));
        assert!(ranges.iter().any(|range| range.start.row >= 2));
        for range in ranges {
            assert_eq!(range.end.column - range.start.column, 5);
        }
    }

    #[test]
    fn reports_only_matches_near_the_viewport_and_follows_scrolling() {
        let mut terminal = Terminal::new(TerminalOptions {
            // Wide rows keep Ghostty pages small relative to the scrollback.
            cols: 200,
            rows: 4,
            max_scrollback: 100_000,
        })
        .unwrap();
        for line in 0..5000 {
            terminal.vt_write(format!("needle {line}\r\n").as_bytes());
        }
        terminal.start_search("needle").unwrap();
        finish_search(&mut terminal);
        let results = terminal.search_results().unwrap();
        // Ghostty retains as much scrollback as its byte budget allows.
        assert!(results.total > 200);
        // Only pages around the viewport: matches near the bottom, none at the top.
        let bottom = terminal.scrollbar().unwrap().offset;
        assert!(results.visible.len() < results.total);
        assert!(
            results
                .visible
                .iter()
                .any(|range| u64::from(range.start.row) >= bottom)
        );
        assert!(!results.visible.iter().any(|range| range.start.row == 0));

        terminal.scroll_viewport(ViewportScroll::Top);
        terminal.search_step(true).unwrap();
        let results = terminal.search_results().unwrap();
        assert!(results.visible.iter().any(|range| range.start.row == 0));
    }

    #[test]
    fn refreshes_results_after_continued_output() {
        let mut terminal = search_terminal();
        terminal.vt_write(b"needle one\r\nskip\r\nneedle two");
        terminal.start_search("needle").unwrap();
        finish_search(&mut terminal);
        let initial_count = terminal.search_results().unwrap().total;

        terminal.vt_write(b"\r\nneedle three");
        finish_search(&mut terminal);

        assert_eq!(terminal.search_results().unwrap().total, initial_count + 1);
    }

    #[test]
    fn navigates_results_in_both_directions_without_scrolling() {
        let mut terminal = search_terminal();
        terminal.vt_write(b"needle one\r\nskip\r\nneedle two");
        terminal.start_search("needle").unwrap();
        finish_search(&mut terminal);
        let viewport = terminal.scrollbar().unwrap();
        assert_eq!(terminal.search_results().unwrap().selected, None);

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

        assert_eq!((newest.0, older.0), (0, 1));
        assert!(older.1.start.row < newest.1.start.row);
        assert_eq!(newest_again, newest);
        assert_eq!(terminal.search_results().unwrap().selected, Some(newest));
        // The widget scrolls to matches itself.
        assert_eq!(terminal.scrollbar().unwrap(), viewport);
    }

    #[test]
    fn selection_wraps_and_empty_results_select_nothing() {
        let mut terminal = search_terminal();
        terminal.vt_write(b"needle one\r\nskip\r\nneedle two");
        terminal.start_search("needle").unwrap();
        finish_search(&mut terminal);
        let newest = terminal.search_select(SearchDirection::Next).unwrap();
        terminal.search_select(SearchDirection::Next).unwrap();
        assert_eq!(
            terminal.search_select(SearchDirection::Next).unwrap(),
            newest
        );

        terminal.start_search("absent").unwrap();
        finish_search(&mut terminal);
        assert_eq!(terminal.search_select(SearchDirection::Next).unwrap(), None);
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
        for _ in 0..100_000 {
            if terminal.search_step(false).unwrap() == SearchProgress::Complete {
                return;
            }
        }
        panic!("search did not finish");
    }
}
