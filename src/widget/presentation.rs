//! The only boundary allowed to copy live terminal state for presentation.
use super::*;
use crate::ghostty::render::{CellWidth, Colors, Cursor, RowSelection};
use crate::ghostty::style::Style;

pub(super) struct FrameCell {
    pub width: CellWidth,
    pub text: String,
    pub foreground: Option<RgbColor>,
    pub background: Option<RgbColor>,
    pub style: Style,
    pub hyperlink: Option<String>,
}

pub(super) struct FrameRow {
    pub cells: Vec<FrameCell>,
    pub selection: Option<RowSelection>,
    pub wrapped: bool,
}
pub(super) struct TerminalFrame {
    pub rows: Vec<FrameRow>,
    pub colors: Colors,
    pub cursor: Cursor,
    pub graphics: graphics::GraphicsFrame,
    pub scrollbar: Scrollbar,
    pub alternate: bool,
    pub selection: Option<[(u16, u32); 2]>,
    pub selection_text: Option<String>,
    pub highlights: Vec<search::SearchHighlight>,
    pub size: (u16, u16),
    pub cell_size: (Pixels, Pixels),
    pub output_seq: u64,
    pub revision: u64,
    pub write_revision: u64,
    pub font_family: String,
    pub font_size_px: f32,
    pub selection_color: RgbColor,
}

pub(super) struct PaintedTerminal {
    pub frame: Rc<TerminalFrame>,
    pub focused: bool,
    pub cursor_phase: bool,
    pub preedit: String,
    pub composing: bool,
}

impl TerminalFrame {
    pub fn cursor_footprint(&self) -> Option<(u16, u16, u16)> {
        let position = self.cursor.position?;
        let column = position.x.saturating_sub(u16::from(position.at_wide_tail));
        let wide = self
            .rows
            .get(usize::from(position.y))
            .and_then(|row| row.cells.get(usize::from(column)))
            .is_some_and(|cell| cell.width == CellWidth::Wide);
        Some((column, position.y, if wide { 2 } else { 1 }))
    }
}

impl TerminalWidget {
    pub(super) fn prepare_terminal(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.resolve_font_metrics(window);
        let size = self
            .layout_bounds
            .map_or(window.viewport_size(), |bounds| bounds.size);
        let _ = self.resize_to_size(size, cx);
        self.publish_terminal(cx);
    }

    /// Start a drain turn of PTY output, which may commit one completed frame
    /// at a hold start.
    pub(super) fn begin_output_turn(&mut self) {
        self.hold_frame_captured = false;
    }

    /// Returns whether presentation is held.
    pub(super) fn poll_presentation_mode(&mut self, cx: &mut Context<Self>) -> bool {
        if self.terminal.render_held() {
            let generation = self.terminal.hold_generation();
            if self.sync_hold.is_none_or(|(old, _)| old != generation) {
                let deadline = cx.background_executor().now() + Duration::from_secs(1);
                self.sync_hold = Some((generation, deadline));
                self.sync_task = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.sync_hold.is_some_and(|(old, deadline)| {
                            old == generation && cx.background_executor().now() >= deadline
                        }) && this.terminal.render_held()
                            && this.terminal.hold_generation() == generation
                        {
                            this.recover_presentation(cx);
                        }
                    });
                });
            }
            return true;
        }
        self.sync_hold = None;
        self.sync_task = Task::ready(());
        false
    }

    /// Commit the frame a program completed before starting a render hold.
    ///
    /// The terminal holds exactly that frame when the hold begins. At most one
    /// capture per drain turn bounds the work when completed frames arrive
    /// faster than they can be shown; later holds keep this complete frame.
    pub(super) fn capture_hold_frame(&mut self, cx: &mut Context<Self>) {
        self.presentation_dirty = true;
        if self.hold_frame_captured || self.presentation_resize_failed {
            return;
        }
        self.hold_frame_captured = true;
        self.commit_terminal_frame(cx);
    }

    pub(super) fn publish_terminal(&mut self, cx: &mut Context<Self>) {
        if self.poll_presentation_mode(cx) || self.presentation_resize_failed {
            return;
        }
        if !self.presentation_dirty
            && self
                .committed
                .as_ref()
                .is_some_and(|frame| frame.write_revision == self.terminal.write_revision())
        {
            return;
        }
        self.commit_terminal_frame(cx);
    }

    fn commit_terminal_frame(&mut self, cx: &mut Context<Self>) {
        match self.build_terminal_frame() {
            Ok(frame) => {
                let blinking_changed = self.committed.as_ref().is_none_or(|old| {
                    (
                        old.cursor.visible,
                        old.cursor.blinking,
                        old.cursor.position.is_some(),
                    ) != (
                        frame.cursor.visible,
                        frame.cursor.blinking,
                        frame.cursor.position.is_some(),
                    )
                });
                // Search highlights are fed per viewport; follow any scroll.
                let scrolled = self
                    .search
                    .as_ref()
                    .is_some_and(|search| search.viewport_offset != frame.scrollbar.offset);
                self.committed = Some(Rc::new(frame));
                if scrolled {
                    self.schedule_search_step(cx);
                }
                self.presentation_dirty = false;
                self.presentation_snapshot_invalid = false;
                self.presentation_retry = Task::ready(());
                if blinking_changed {
                    self.schedule_cursor_blink(cx);
                }
                cx.notify();
            }
            Err(error) => {
                self.presentation_failure(error);
                self.presentation_dirty = true;
                self.presentation_snapshot_invalid = true;
                self.presentation_retry = cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                    let _ = this.update(cx, |this, cx| this.publish_terminal(cx));
                });
            }
        }
    }

    pub(super) fn recover_presentation(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.terminal.end_synchronized_output() {
            self.presentation_failure(error);
            return;
        }
        self.sync_hold = None;
        self.sync_task = Task::ready(());
        self.presentation_dirty = true;
        self.publish_terminal(cx);
    }

    fn presentation_failure(&self, error: crate::ghostty::Error) {
        crate::diagnostics::record(
            "presentation",
            "build_failed",
            &error.to_string(),
            launch_context(&self.config.launch),
        );
    }

    fn build_terminal_frame(&mut self) -> crate::ghostty::Result<TerminalFrame> {
        // A failed native update may already have consumed some row dirty
        // flags, leaving cached rows stale; a fresh state copies every row.
        if self.presentation_snapshot_invalid {
            self.render_state = RenderState::new()?;
        }
        let snapshot = self.render_state.update(&self.terminal)?;
        let colors = snapshot.colors()?;
        let cursor = snapshot.cursor()?;
        let mut rows = Vec::with_capacity(usize::from(self.size.1));
        let mut row_it = self.row_iterator.update(&snapshot)?;
        while let Some(row) = row_it.next() {
            let selection = row.selection()?;
            let mut cells = Vec::with_capacity(usize::from(self.size.0));
            let mut cell_it = self.cell_iterator.update(row)?;
            while let Some(cell) = cell_it.next() {
                cells.push(FrameCell {
                    width: cell.width()?,
                    text: cell.text()?,
                    foreground: cell.fg_color()?,
                    background: cell.bg_color()?,
                    style: cell.style()?,
                    hyperlink: if cell.has_hyperlink()? {
                        self.terminal
                            .hyperlink_uri(cells.len() as u16, rows.len() as u32)?
                    } else {
                        None
                    },
                });
            }
            rows.push(FrameRow {
                cells,
                selection,
                wrapped: self.terminal.viewport_row_wrapped(rows.len() as u16)?,
            });
        }
        let scrollbar = self.terminal.scrollbar()?;
        let highlights = self
            .search
            .as_ref()
            .filter(|search| search.output_revision == self.terminal.write_revision())
            .filter(|search| search.geometry == self.size)
            .map(|search| search.visible_highlights(scrollbar, self.size.0, self.size.1))
            .unwrap_or_default();
        Ok(TerminalFrame {
            rows,
            colors,
            cursor,
            scrollbar,
            highlights,
            alternate: self.terminal.active_buffer_is_alternate()?,
            selection: self.terminal.selection_coordinates()?,
            selection_text: self.terminal.selected_text()?,
            graphics: self
                .graphics_renderer
                .frame(&self.terminal, self.cell_size)?,
            size: self.size,
            cell_size: self.cell_size,
            output_seq: self.output_seq,
            write_revision: self.terminal.write_revision(),
            font_family: self.config.font_family.clone(),
            font_size_px: self.config.font_size_px,
            selection_color: rgba_to_rgb(self.theme.selection),
            revision: self
                .committed
                .as_ref()
                .map_or(1, |old| old.revision.wrapping_add(1)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    fn widget(cx: &mut TestAppContext) -> gpui_kit::Entity<TerminalWidget> {
        cx.new(|cx| {
            TerminalWidget::with_pty(
                TerminalConfig {
                    initial_cols: 24,
                    initial_rows: 4,
                    cursor_blink: false,
                    ..Default::default()
                },
                Arc::new(AtomicBool::new(false)),
                None,
                None,
                None,
                cx,
            )
        })
    }
    fn write(widget: &gpui_kit::Entity<TerminalWidget>, bytes: &[u8], cx: &mut TestAppContext) {
        widget.update(cx, |widget, cx| {
            widget.begin_output_turn();
            widget.apply_pty_event(PtyEvent::Output(bytes.to_vec()), cx);
            widget.publish_terminal(cx);
        });
    }
    fn text(frame: &TerminalFrame) -> String {
        frame
            .rows
            .iter()
            .flat_map(|row| &row.cells)
            .map(|cell| cell.text.as_str())
            .collect()
    }

    fn appearance(frame: &TerminalFrame) -> serde_json::Value {
        let mut capture = super::super::capture::feedback_capture(frame);
        capture.captured_unix_ms = 0;
        serde_json::json!({"capture":capture,"cursor":format!("{:?}",frame.cursor),
            "palette":frame.colors.palette.map(|c| (c.r,c.g,c.b)).to_vec(),
            "alternate":frame.alternate,"viewport":(frame.scrollbar.offset,frame.scrollbar.len,frame.scrollbar.total)})
    }

    #[gpui_kit::test]
    fn every_split_and_random_chunking_retains_complete_frames(cx: &mut TestAppContext) {
        let small = b"\x1b[?2026h\r\x1b[2Knew\x1b[?25l\x1b[3;8H\x1b[?2026l";
        let widget = widget(cx);
        for split in 0..=small.len() {
            write(&widget, b"\x1bcold", cx);
            let before = widget.read_with(cx, |widget, _| widget.committed.clone().unwrap());
            write(&widget, &small[..split], cx);
            widget.read_with(cx, |widget, _| {
                if widget.terminal.synchronized_output().unwrap() {
                    assert!(
                        Rc::ptr_eq(&before, widget.committed.as_ref().unwrap()),
                        "split {split}"
                    );
                }
            });
            write(&widget, &small[split..], cx);
            widget.read_with(cx, |widget, _| {
                let frame = widget.committed.as_ref().unwrap();
                assert!(text(frame).contains("new"));
                assert!(!frame.cursor.visible);
                assert_eq!(frame.output_seq, widget.output_seq);
            });
        }
        let frames = (0..80)
            .map(|index| {
                format!(
                    "\x1b[?2026h\x1b[?25l\x1b[H\x1b[2Kframe-{index:03}\x1b[2;1H\x1b[1;38;2;123;80;40m界e\u{301}😀\x1b[0m\x1b]8;;https://example.com\x1b\\label\x1b]8;;\x1b\\\x1b[3;8H\x1b[6 q\x1b[?25h\x1b[?2026l"
                )
                .into_bytes()
            })
            .collect::<Vec<_>>();
        let stream = frames.concat();
        // Every frame the program completes; nothing else may be committed.
        write(&widget, b"\x1bc", cx);
        let committed = |cx: &mut TestAppContext| {
            widget.read_with(cx, |widget, _| {
                appearance(widget.committed.as_ref().unwrap()).to_string()
            })
        };
        let mut complete = std::collections::HashSet::from([committed(cx)]);
        for frame in &frames {
            write(&widget, frame, cx);
            complete.insert(committed(cx));
        }
        let expected = committed(cx);
        // Every unfinished state here differs from complete frames in its text
        // or cursor (hidden until the frame ends), so compare a whole frame
        // the first time its text and cursor are committed.
        let mut verified = std::collections::HashSet::new();
        let mut assert_complete = |cx: &mut TestAppContext, context: &str| {
            let key = widget.read_with(cx, |widget, _| {
                let frame = widget.committed.as_ref().unwrap();
                format!("{}{:?}", text(frame), frame.cursor)
            });
            if verified.insert(key) {
                assert!(complete.contains(&committed(cx)), "{context}");
            }
        };
        for chunk_size in [1, 2, 7, 31, 1024, stream.len()] {
            write(&widget, b"\x1bc", cx);
            for chunk in stream.chunks(chunk_size) {
                write(&widget, chunk, cx);
                assert_complete(cx, &format!("chunk size {chunk_size}"));
            }
            assert_eq!(committed(cx), expected);
        }
        for seed in 1..=12u64 {
            write(&widget, b"\x1bc", cx);
            let mut random = seed;
            let mut remaining = stream.as_slice();
            while !remaining.is_empty() {
                random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                let count = (1 + (random as usize % 113)).min(remaining.len());
                write(&widget, &remaining[..count], cx);
                assert_complete(cx, &format!("seed {seed}"));
                remaining = &remaining[count..];
            }
            assert_eq!(committed(cx), expected);
        }
    }

    #[gpui_kit::test]
    fn continuous_holds_publish_each_completed_frame(cx: &mut TestAppContext) {
        let widget = widget(cx);
        write(&widget, b"\x1b[?2026h\rf0", cx);
        // Every drain turn completes one frame and starts the next hold.
        for frame in 1..=5 {
            cx.background_executor
                .advance_clock(Duration::from_millis(400));
            cx.run_until_parked();
            write(
                &widget,
                format!("\x1b[?2026l\x1b[?2026h\r\x1b[2Kf{frame}").as_bytes(),
                cx,
            );
            widget.read_with(cx, |widget, _| {
                assert!(widget.terminal.synchronized_output().unwrap());
                let previous = format!("f{}", frame - 1);
                assert!(
                    text(widget.committed.as_ref().unwrap()).starts_with(&previous),
                    "frame {frame} did not publish {previous}"
                );
            });
        }
    }

    #[gpui_kit::test]
    fn output_task_commits_completed_frames_in_every_drain_turn(cx: &mut TestAppContext) {
        let (tx, rx) = flume::bounded(64);
        let widget = cx.new(|cx| {
            TerminalWidget::with_pty(
                TerminalConfig {
                    initial_cols: 24,
                    initial_rows: 4,
                    cursor_blink: false,
                    ..Default::default()
                },
                Arc::new(AtomicBool::new(false)),
                None,
                Some(rx),
                None,
                cx,
            )
        });
        tx.send(PtyEvent::Output(b"\x1b[?2026h\rf0".to_vec()))
            .unwrap();
        cx.run_until_parked();
        for frame in 1..=3 {
            tx.send(PtyEvent::Output(
                format!("\x1b[?2026l\x1b[?2026h\r\x1b[2Kf{frame}").into_bytes(),
            ))
            .unwrap();
            // The task yields on a 1 ms timer between drain turns.
            cx.background_executor
                .advance_clock(Duration::from_millis(1));
            cx.run_until_parked();
            widget.read_with(cx, |widget, _| {
                assert!(widget.terminal.render_held());
                let previous = format!("f{}", frame - 1);
                assert!(text(widget.committed.as_ref().unwrap()).starts_with(&previous));
            });
        }
    }

    #[gpui_kit::test]
    fn scrolling_refeeds_search_highlights_without_a_repaint(cx: &mut TestAppContext) {
        let widget = widget(cx);
        let mut lines = String::new();
        for line in 0..400 {
            lines.push_str(&format!("needle {line}\r\nfiller\r\n"));
        }
        write(&widget, lines.as_bytes(), cx);
        widget.update(cx, |widget, cx| {
            widget.terminal.start_search("needle").unwrap();
            widget.search = Some(search::SearchOverlay {
                query: "needle".into(),
                progress: SearchProgress::Pending,
                ..Default::default()
            });
            widget.schedule_search_step(cx);
        });
        for _ in 0..1000 {
            cx.background_executor
                .advance_clock(Duration::from_millis(1));
            cx.run_until_parked();
            if widget.read_with(cx, |widget, _| {
                widget.search.as_ref().unwrap().progress == SearchProgress::Complete
            }) {
                break;
            }
        }
        widget.update(cx, |widget, cx| {
            assert_ne!(widget.search.as_ref().unwrap().viewport_offset, 0);
            widget.scroll_viewport(ViewportScroll::Top, cx);
        });
        // Only the search step runs; nothing publishes a frame in between.
        cx.background_executor
            .advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        widget.read_with(cx, |widget, _| {
            let search = widget.search.as_ref().unwrap();
            assert_eq!(search.viewport_offset, 0);
            assert!(
                search
                    .results
                    .visible
                    .iter()
                    .any(|range| range.start.row == 0)
            );
        });
    }

    #[gpui_kit::test]
    fn hold_start_does_not_commit_while_a_resize_failed(cx: &mut TestAppContext) {
        let widget = widget(cx);
        write(&widget, b"before", cx);
        let before = widget.read_with(cx, |widget, _| widget.committed.clone().unwrap());
        widget.update(cx, |widget, cx| {
            widget.presentation_resize_failed = true;
            widget.begin_output_turn();
            widget.apply_pty_event(
                PtyEvent::Output(b"\r\x1b[2Kafter\x1b[?2026h\r\x1b[2Kpartial".to_vec()),
                cx,
            );
            assert!(Rc::ptr_eq(&before, widget.committed.as_ref().unwrap()));
            assert!(widget.presentation_dirty);
        });
    }

    #[gpui_kit::test]
    fn split_hold_start_does_not_rebuild_an_unchanged_frame(cx: &mut TestAppContext) {
        let widget = widget(cx);
        write(&widget, b"before", cx);
        let before = widget.read_with(cx, |widget, _| widget.committed.clone().unwrap());
        write(&widget, b"\x1b[?20", cx);
        write(&widget, b"26h\r\x1b[2Kpartial", cx);
        widget.read_with(cx, |widget, _| {
            assert!(widget.terminal.render_held());
            assert!(Rc::ptr_eq(&before, widget.committed.as_ref().unwrap()));
        });
    }

    #[gpui_kit::test]
    fn completed_frame_floods_capture_once_per_drain_turn(cx: &mut TestAppContext) {
        let widget = widget(cx);
        write(&widget, b"start", cx);
        let before = widget.read_with(cx, |widget, _| widget.committed.as_ref().unwrap().revision);
        let mut flood = Vec::new();
        for frame in 0..50 {
            flood.extend_from_slice(
                format!("\x1b[?2026h\r\x1b[2Kf{frame:02}\x1b[?2026l").as_bytes(),
            );
        }
        flood.extend_from_slice(b"\x1b[?2026h\r\x1b[2Kunfinished");
        write(&widget, &flood, cx);
        widget.read_with(cx, |widget, _| {
            let frame = widget.committed.as_ref().unwrap();
            assert_eq!(frame.revision, before + 1);
            assert!(text(frame).starts_with("f00"));
        });
        write(&widget, b"\x1b[?2026l", cx);
        widget.read_with(cx, |widget, _| {
            assert!(text(widget.committed.as_ref().unwrap()).starts_with("unfinished"))
        });
    }

    #[gpui_kit::test]
    fn timeout_obsolete_timers_resize_reset_and_eof(cx: &mut TestAppContext) {
        let widget = widget(cx);
        write(&widget, b"old", cx);
        write(&widget, b"\x1b[?2026h\rnew", cx);
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(Duration::from_millis(700));
        cx.run_until_parked();
        // End and restart in one chunk: the frame completed between the holds
        // is shown, and the old deadline must not end the new hold.
        write(&widget, b"\x1b[?2026l\x1b[?2026h\rfresh", cx);
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        widget.read_with(cx, |widget, _| {
            assert!(widget.terminal.synchronized_output().unwrap());
            assert!(text(widget.committed.as_ref().unwrap()).starts_with("new"));
        });
        cx.background_executor
            .advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
        widget.read_with(cx, |widget, _| {
            assert!(!widget.terminal.synchronized_output().unwrap());
            assert!(text(widget.committed.as_ref().unwrap()).contains("fresh"));
        });
        write(&widget, b"\x1b[?2026hresize", cx);
        widget.update(cx, |widget, cx| {
            widget.resize_to_size(gpui_kit::size(px(200.), px(100.)), cx);
            let frame = widget.committed.as_ref().unwrap();
            assert!(!widget.terminal.synchronized_output().unwrap());
            assert_eq!(frame.size, widget.size);
            assert_eq!(frame.cell_size, widget.cell_size);
        });
        write(&widget, b"\x1b[?2026hheld\x1bctreset", cx);
        widget.read_with(cx, |widget, _| {
            assert!(text(widget.committed.as_ref().unwrap()).contains("treset"))
        });
        write(&widget, b"\x1b[?2026h\rfinal", cx);
        widget.update(cx, |widget, cx| {
            widget.apply_pty_event(PtyEvent::OutputEnded(None), cx);
            assert!(text(widget.committed.as_ref().unwrap()).contains("final"));
            assert!(!widget.terminal.synchronized_output().unwrap());
        });
    }

    #[gpui_kit::test]
    fn replies_input_graphics_and_capture_remain_coherent_during_hold(cx: &mut TestAppContext) {
        let (tx, rx) = pty::test_channel();
        let widget = cx.new(|cx| {
            TerminalWidget::with_pty(
                TerminalConfig {
                    cursor_blink: false,
                    ..Default::default()
                },
                Arc::new(AtomicBool::new(false)),
                None,
                None,
                Some(tx),
                cx,
            )
        });
        widget.update(cx, |widget, cx| {
            widget.resize_to_size(gpui_kit::size(px(600.), px(400.)), cx);
        });
        write(&widget, b"base", cx);
        widget.update(cx, |widget, cx| {
            let cell_width = f64::from(f32::from(widget.cell_size.0));
            let point = |column| SelectionPoint {
                column,
                row: 0,
                surface_x: cell_width * f64::from(column) + 1.0,
                surface_y: 1.0,
            };
            widget
                .terminal
                .selection_press(SelectionPress {
                    point: point(0),
                    time_ns: 1,
                    repeat_interval_ns: CLICK_REPEAT_INTERVAL_NS,
                    repeat_distance: 10.0,
                })
                .unwrap();
            widget
                .terminal
                .selection_drag(SelectionDrag {
                    point: point(3),
                    geometry: widget.selection_geometry(),
                    rectangle: false,
                })
                .unwrap();
            widget.terminal.selection_release(Some(point(3))).unwrap();
            widget.terminal.start_search("base").unwrap();
            while widget.terminal.search_step(false).unwrap() == SearchProgress::Pending {}
            widget.search = Some(search::SearchOverlay {
                query: "base".into(),
                results: widget.terminal.search_results().unwrap(),
                output_revision: widget.terminal.write_revision(),
                geometry: widget.size,
                ..Default::default()
            });
            widget.presentation_dirty = true;
            widget.publish_terminal(cx);
            assert!(widget.committed.as_ref().unwrap().selection.is_some());
            assert!(!widget.committed.as_ref().unwrap().highlights.is_empty());
        });
        let before = widget.read_with(cx, |widget, _| widget.committed.clone().unwrap());
        write(
            &widget,
            b"\x1b[?2026h\x1b[2J\x1b[Hchanged\x1b[6n\x1b_Ga=T,t=d,f=24,i=1,p=1,s=1,v=1;////\x1b\\",
            cx,
        );
        assert!(
            rx.try_iter().any(
                |command| matches!(command, PtyCommand::Write(bytes) if bytes == b"\x1b[1;8R")
            )
        );
        widget.update(cx, |widget, cx| {
            widget.send_encoded_key(Action::Press, &Keystroke::parse("enter").unwrap(), cx);
            widget.cursor_blink_phase = false;
            widget.terminal.clear_selection().unwrap();
            widget.presentation_dirty = true;
            widget.publish_terminal(cx);
            let capture = widget.build_feedback_capture(true).unwrap();
            assert!(capture.rows.iter().any(|row| row.text.contains("base")));
            assert!(Rc::ptr_eq(&before, widget.committed.as_ref().unwrap()));
            assert!(widget.committed.as_ref().unwrap().selection.is_some());
            assert!(!widget.committed.as_ref().unwrap().highlights.is_empty());
            assert!(
                widget
                    .committed
                    .as_ref()
                    .unwrap()
                    .graphics
                    .above_text
                    .is_empty()
            );
        });
        assert!(
            rx.try_iter()
                .any(|command| matches!(command, PtyCommand::Write(bytes) if bytes == b"\r"))
        );
        write(&widget, b"\x1b[?2026l", cx);
        widget.update(cx, |widget, _| {
            let frame = widget.committed.as_ref().unwrap();
            assert_eq!(frame.graphics.above_text.len(), 1);
            assert_eq!(frame.output_seq, widget.output_seq);
            assert!(frame.selection.is_none());
            assert!(
                frame.highlights.is_empty(),
                "stale search ranges survived new output"
            );
            let capture = widget.build_feedback_capture(false).unwrap();
            assert!(capture.rows.iter().any(|row| row.text.contains("changed")));
        });
    }

    #[gpui_kit::test]
    fn recovery_recopies_rows_after_dirty_flags_were_consumed(cx: &mut TestAppContext) {
        let widget = widget(cx);
        write(&widget, b"before", cx);
        widget.update(cx, |widget, cx| {
            widget.apply_pty_event(PtyEvent::Output(b"\r\x1b[2Kafter".to_vec()), cx);
            // Model an update that consumed terminal dirtiness but left our cached rows stale.
            RenderState::new()
                .unwrap()
                .update(&widget.terminal)
                .unwrap();
            widget.presentation_snapshot_invalid = true;
            widget.publish_terminal(cx);
            let frame = widget.committed.as_ref().unwrap();
            assert!(text(frame).starts_with("after"));
            assert_eq!(frame.output_seq, widget.output_seq);
            assert!(!widget.presentation_snapshot_invalid);
        });
    }

    #[gpui_kit::test]
    fn pending_resize_retains_matching_rows_and_metrics(cx: &mut TestAppContext) {
        let widget = widget(cx);
        write(&widget, b"before", cx);
        let before = widget.read_with(cx, |widget, _| widget.committed.clone().unwrap());
        widget.update(cx, |widget, cx| {
            widget.cell_size = (px(12.), px(24.));
            widget.geometry_dirty = true;
            widget.presentation_resize_failed = true;
            widget.apply_pty_event(PtyEvent::Output(b"\r\x1b[2Kafter".to_vec()), cx);
            widget.publish_terminal(cx);
            assert!(Rc::ptr_eq(&before, widget.committed.as_ref().unwrap()));
            widget.resize_to_size(gpui_kit::size(px(240.), px(120.)), cx);
            let frame = widget.committed.as_ref().unwrap();
            assert!(text(frame).starts_with("after"));
            assert_eq!(frame.cell_size, (px(12.), px(24.)));
            assert_eq!(frame.size, widget.size);
            assert!(!widget.presentation_resize_failed);
        });
    }

    #[gpui_kit::test]
    fn exhausted_image_budget_retains_frame_and_retries_without_output(cx: &mut TestAppContext) {
        let widget = widget(cx);
        widget.update(cx, |widget, cx| {
            widget.resize_to_size(gpui_kit::size(px(600.), px(400.)), cx);
        });
        write(&widget, b"before", cx);
        let before = widget.read_with(cx, |widget, _| widget.committed.clone().unwrap());
        widget.update(cx, |widget, _| widget.graphics_renderer.test_cache_limit(0));
        write(
            &widget,
            b"\r\x1b[2Kafter\x1b_Ga=T,t=d,f=24,i=1,p=1,s=1,v=1;////\x1b\\",
            cx,
        );
        widget.update(cx, |widget, _| {
            assert!(Rc::ptr_eq(&before, widget.committed.as_ref().unwrap()));
            assert!(widget.presentation_dirty);
            widget.graphics_renderer.test_cache_limit(1024);
        });
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        widget.read_with(cx, |widget, _| {
            let frame = widget.committed.as_ref().unwrap();
            assert!(text(frame).contains("after"));
            assert_eq!(frame.graphics.above_text.len(), 1);
            assert_eq!(frame.output_seq, widget.output_seq);
            assert!(!widget.presentation_dirty);
        });
    }
}
