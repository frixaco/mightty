//! In-pane search editor state and viewport highlight projection.

use crate::ghostty::{Scrollbar, SearchProgress, SearchRange};

pub(super) const MAX_SEARCH_QUERY_BYTES: usize = 4096;

pub(super) struct SearchOverlay {
    pub query: String,
    pub ranges: Vec<SearchRange>,
    pub active: Option<SearchRange>,
    pub progress: SearchProgress,
    pub diagnostic: Option<String>,
    pub revision: u64,
    pub step_scheduled: bool,
    pub output_revision: u64,
    pub geometry: (u16, u16),
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
            ranges: Vec::new(),
            active: None,
            progress: SearchProgress::Complete,
            diagnostic: None,
            revision: 0,
            step_scheduled: false,
            output_revision: 0,
            geometry: (0, 0),
        }
    }
}

impl SearchOverlay {
    pub fn result_label(&self) -> String {
        let current = self
            .active
            .and_then(|active| self.ranges.iter().position(|range| *range == active))
            .map_or(0, |index| index + 1);
        let progress = match self.progress {
            SearchProgress::Pending => " · Searching",
            SearchProgress::Complete => "",
        };
        format!("{current}/{}{progress}", self.ranges.len())
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

        for range in &self.ranges {
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
                    active: self.active == Some(*range),
                });
            }
        }
        highlights
    }
}

#[cfg(test)]
mod tests {
    use crate::ghostty::SearchPoint;

    use super::*;

    #[test]
    fn projects_wrapped_ranges_into_the_visible_viewport() {
        let active = range(0, 9, 2, 11);
        let overlay = SearchOverlay {
            ranges: vec![active, range(3, 12, 8, 12), range(0, 20, 1, 20)],
            active: Some(active),
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
    fn labels_the_active_result_and_pending_work() {
        let first = range(0, 2, 3, 2);
        let active = range(0, 4, 3, 4);
        let overlay = SearchOverlay {
            ranges: vec![first, active],
            active: Some(active),
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
