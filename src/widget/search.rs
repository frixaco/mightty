//! In-pane search editor state and viewport highlight projection.

use crate::ghostty::{Scrollbar, SearchProgress, SearchResults};

pub(super) const MAX_SEARCH_QUERY_BYTES: usize = 4096;

pub(super) struct SearchOverlay {
    pub query: String,
    pub results: SearchResults,
    pub progress: SearchProgress,
    pub diagnostic: Option<String>,
    pub revision: u64,
    pub step_scheduled: bool,
    pub output_revision: u64,
    pub geometry: (u16, u16),
    /// Viewport offset the results were fed at; scrolling needs a new feed.
    pub viewport_offset: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SearchHighlight {
    pub row: u16,
    pub start_column: u16,
    pub end_column: u16,
    pub active: bool,
}

impl Default for SearchOverlay {
    fn default() -> Self {
        Self {
            query: String::new(),
            results: SearchResults::default(),
            progress: SearchProgress::Complete,
            diagnostic: None,
            revision: 0,
            step_scheduled: false,
            output_revision: 0,
            geometry: (0, 0),
            viewport_offset: 0,
        }
    }
}

impl SearchOverlay {
    pub fn result_label(&self) -> String {
        let current = self.results.selected.map_or(0, |(index, _)| index + 1);
        let progress = match self.progress {
            SearchProgress::Pending => " · Searching",
            SearchProgress::Complete => "",
        };
        format!("{current}/{}{progress}", self.results.total)
    }

    pub fn visible_highlights(
        &self,
        scrollbar: Scrollbar,
        columns: u16,
        rows: u16,
    ) -> Vec<SearchHighlight> {
        if columns == 0 || rows == 0 || scrollbar.len == 0 {
            return Vec::new();
        }
        let visible_start = scrollbar.offset;
        let visible_end = visible_start.saturating_add(scrollbar.len.min(u64::from(rows)));
        let last_column = columns - 1;
        let mut highlights = Vec::new();
        let active = self.results.selected.map(|(_, range)| range);
        // The selected match is drawn even when its page was not fed yet.
        let ranges = self.results.visible.iter().chain(
            active
                .iter()
                .filter(|active| !self.results.visible.contains(active)),
        );

        for range in ranges {
            if range.start.row > range.end.row
                || (range.start.row == range.end.row && range.start.column > range.end.column)
            {
                continue;
            }
            let first_row = u64::from(range.start.row).max(visible_start);
            let last_row = u64::from(range.end.row).saturating_add(1).min(visible_end);
            for row in first_row..last_row {
                let row_u32 = row as u32;
                let start_column = if row_u32 == range.start.row {
                    range.start.column
                } else {
                    0
                };
                if start_column >= columns {
                    continue;
                }
                let end_column = if row_u32 == range.end.row {
                    range.end.column.min(last_column)
                } else {
                    last_column
                };
                if start_column > end_column {
                    continue;
                }
                highlights.push(SearchHighlight {
                    row: (row - visible_start) as u16,
                    start_column,
                    end_column,
                    active: active == Some(*range),
                });
            }
        }
        highlights
    }
}

#[cfg(test)]
mod tests {
    use crate::ghostty::{SearchPoint, SearchRange};

    use super::*;

    #[test]
    fn projects_wrapped_ranges_into_the_visible_viewport() {
        let active = range(0, 9, 2, 11);
        let overlay = SearchOverlay {
            results: SearchResults {
                total: 3,
                selected: Some((0, active)),
                visible: vec![active, range(3, 12, 8, 12), range(0, 20, 1, 20)],
            },
            ..Default::default()
        };

        assert_eq!(
            overlay.visible_highlights(
                Scrollbar {
                    total: 30,
                    offset: 10,
                    len: 3,
                },
                5,
                3,
            ),
            [
                SearchHighlight {
                    row: 0,
                    start_column: 0,
                    end_column: 4,
                    active: true,
                },
                SearchHighlight {
                    row: 1,
                    start_column: 0,
                    end_column: 2,
                    active: true,
                },
                SearchHighlight {
                    row: 2,
                    start_column: 3,
                    end_column: 4,
                    active: false,
                },
            ]
        );
    }

    #[test]
    fn draws_the_selected_match_before_its_page_is_fed() {
        let active = range(1, 10, 2, 10);
        let overlay = SearchOverlay {
            results: SearchResults {
                total: 1,
                selected: Some((0, active)),
                visible: Vec::new(),
            },
            ..Default::default()
        };
        let scrollbar = Scrollbar {
            total: 30,
            offset: 10,
            len: 3,
        };
        assert_eq!(
            overlay.visible_highlights(scrollbar, 5, 3),
            [SearchHighlight {
                row: 0,
                start_column: 1,
                end_column: 2,
                active: true,
            }]
        );
    }

    #[test]
    fn labels_the_active_result_and_pending_work() {
        let overlay = SearchOverlay {
            results: SearchResults {
                total: 2,
                selected: Some((1, range(0, 4, 3, 4))),
                visible: Vec::new(),
            },
            progress: SearchProgress::Pending,
            ..Default::default()
        };

        assert_eq!(overlay.result_label(), "2/2 · Searching");
    }

    fn range(start_column: u16, start_row: u32, end_column: u16, end_row: u32) -> SearchRange {
        SearchRange {
            start: SearchPoint {
                column: start_column,
                row: start_row,
            },
            end: SearchPoint {
                column: end_column,
                row: end_row,
            },
        }
    }
}
