use crate::feedback::{
    self, CaptureCell, CaptureColors, CaptureCursor, CaptureRow, FontCapture, GridSize, RgbHex,
    SizePx, TerminalCapture,
};
use crate::ghostty::{
    render::CellWidth,
    style::{RgbColor, Underline},
};

use super::TerminalWidget;
use super::render::CellWidthExt;

impl TerminalWidget {
    pub fn capture_cell_count(&self) -> usize {
        usize::from(self.size.0) * usize::from(self.size.1)
    }
    pub fn geometry_ready(&self) -> bool {
        self.pty_tx
            .as_ref()
            .and_then(|tx| tx.acknowledged_size())
            .is_some_and(|size| (size.cols, size.rows) == self.size)
            && self.layout_bounds.is_some()
    }
    pub fn presentation(
        &mut self,
        window: &gpui::Window,
        offscreen: bool,
        include_source: bool,
    ) -> crate::snapshot::PaneFrame {
        let key = format!(
            "{:?}",
            (
                self.output_seq,
                offscreen,
                self.size,
                self.terminal.scrollbar().ok().map(|s| s.offset),
                self.terminal.selection_coordinates().ok(),
                &self.config.font_family,
                self.config.font_size_px,
                self.cell_size,
                &self.theme
            )
        );
        let source = if !include_source || self.capture_cell_count() > 20000 {
            self.capture_cache = None;
            None
        } else {
            if self
                .capture_cache
                .as_ref()
                .is_none_or(|(old, _)| *old != key)
            {
                self.capture_cache = self
                    .build_feedback_capture(offscreen)
                    .ok()
                    .map(|capture| (key, std::sync::Arc::new(capture)));
            }
            self.capture_cache
                .as_ref()
                .map(|(_, capture)| capture.clone())
        };
        let mut state = self.control_state();
        for field in [
            "processes",
            "lifecycle",
            "output_eof",
            "io_error",
            "pty_size",
        ] {
            state.as_object_mut().unwrap().remove(field);
        }
        state["focus_appearance"] = serde_json::json!(self.focus_handle.is_focused(window));
        state["cursor_phase"] = serde_json::json!(self.cursor_blink_phase);
        state["preedit"] = serde_json::json!(self.preedit);
        state["pending_paste_confirmation"] = serde_json::json!(self.pending_paste.is_some());
        state["selection_coordinates"] =
            serde_json::json!(self.terminal.selection_coordinates().ok().flatten());
        state["effective_settings"] = serde_json::json!({"font_family":self.config.font_family,"font_size_px":self.config.font_size_px,"cursor_style":format!("{:?}",self.config.cursor_style),"cursor_blink":self.config.cursor_blink,"blink_interval_ms":self.config.blink_interval.as_millis().to_string(),"scrollback":self.config.scrollback,"theme":format!("{:?}",self.theme),"clipboard_policy":format!("{:?}",self.config.terminal_clipboard_policy),"action_bindings":self.config.action_bindings});
        state["launch"]["inherit_environment"] =
            serde_json::json!(self.config.launch.inherit_environment);
        state["launch"]["unset_environment"] = serde_json::json!(
            self.config
                .launch
                .unset_environment
                .iter()
                .map(|v| v.to_string_lossy())
                .collect::<Vec<_>>()
        );
        state["launch"]["shell_integration"] =
            serde_json::json!(self.config.launch.shell_integration);
        state["launch"]["environment_values"] = serde_json::json!(
            self.config
                .launch
                .environment
                .iter()
                .map(|(key, value)| (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned()
                ))
                .collect::<std::collections::BTreeMap<_, _>>()
        );
        state["painter"] = serde_json::json!({"font_features":format!("{:?}",super::render::terminal_font_features()),"fallback_chain":format!("{:?}",super::render::terminal_font_fallbacks(&self.config.font_family)),"resolved_font_faces":{"availability":"unavailable","reason":"GPUI does not retain face/cluster diagnostics for this surface"}});
        crate::snapshot::PaneFrame { state, source }
    }
    pub fn control_root_process(&self) -> Option<crate::diagnostics::RootProcess> {
        self.pty_worker
            .as_ref()
            .and_then(|worker| worker.root_process())
    }
    pub fn control_register_search(&mut self, query: &str) -> crate::ghostty::Result<u64> {
        self.terminal.register_search(query)
    }
    pub fn control_release_search(&mut self, id: u64) {
        self.terminal.release_search(id);
    }
    pub fn control_probe_search(&mut self, id: u64) -> crate::ghostty::Result<(bool, bool)> {
        self.terminal.probe_search(id)
    }
    pub fn control_prompt_ready(&self) -> Result<bool, crate::control::ControlError> {
        if !self.semantic_commands_available {
            return Err(crate::control::ControlError::new(
                "unavailable",
                "shell prompt integration has not been observed",
            ));
        }
        Ok(self.terminal.cursor_at_prompt()?)
    }
    pub fn control_state(&self) -> serde_json::Value {
        use serde_json::json;
        let bounds = self.layout_bounds.map(|b| json!({"x":f32::from(b.origin.x),"y":f32::from(b.origin.y),"width":f32::from(b.size.width),"height":f32::from(b.size.height)}));
        json!({"title":{"reported":self.reported_title,"normalized":crate::shell_integration::display_title(self.reported_title.as_deref())},
            "working_directory":{"configured":self.config.launch.working_directory,"reported":self.reported_working_directory},
            "launch":{"executable":self.config.launch.executable,"argv":self.config.launch.arguments.iter().map(|v|v.to_string_lossy()).collect::<Vec<_>>(),
                "environment_names":self.config.launch.environment.keys().map(|v|v.to_string_lossy()).collect::<Vec<_>>()},
            "terminal_size":{"cols":self.size.0,"rows":self.size.1},"computed_bounds":bounds,
            "pty_size":self.pty_tx.as_ref().and_then(|tx|tx.acknowledged_size()).map(|s|json!({"cols":s.cols,"rows":s.rows})),
            "output_seq":self.output_seq.to_string(),"output_cursor":format!("{}:{}",crate::control::instance_id(),self.output_seq),
            "lifecycle":if self.has_exited {"output_ended"} else {"running"},"output_eof":self.output_eof,"io_error":self.io_error,"processes":self.pty_worker.as_ref().and_then(|worker|worker.root_process()).map(|root|root.state()).unwrap_or(serde_json::json!({"availability":"unavailable"})),
            "font":{"family":self.config.font_family,"size_px":self.config.font_size_px},
            "viewport":self.terminal.scrollbar().ok().map(|s|json!({"offset":s.offset,"length":s.len,"total":s.total})),
            "selection":self.has_selection(),"search_open":self.search.is_some(),"search":self.search.as_ref().map(|search|json!({"query":search.query,"progress":format!("{:?}",search.progress),"matches":search.ranges.len(),"diagnostic":search.diagnostic}))})
    }

    pub fn control_read(
        &mut self,
        request: &crate::control::Request,
    ) -> Result<serde_json::Value, crate::control::ControlError> {
        use crate::control::{ControlError, number_arg, string_arg};
        let alternate = self.terminal.active_buffer_is_alternate()?;
        if string_arg(request, "buffer")?
            .is_some_and(|b| b != "active" && b != if alternate { "alternate" } else { "primary" })
        {
            return Err(ControlError::new(
                "unsupported_buffer",
                "requested buffer is inactive; switching it would change the terminal",
            ));
        }
        let format = string_arg(request, "format")?.unwrap_or("text");
        if !matches!(format, "text" | "cells") {
            return Err(ControlError::new(
                "invalid_argument",
                "format must be text or cells",
            ));
        }
        let tail = number_arg(request, "tail", crate::control::MAX_READ_ROWS as f64)?;
        if tail < 1.0 || tail.fract() != 0.0 {
            return Err(ControlError::new(
                "invalid_argument",
                "tail must be a positive integer",
            ));
        }
        let viewport =
            crate::control::bool_arg(request, "viewport", !request.args.contains_key("tail"))?;
        if viewport && request.args.contains_key("tail") {
            return Err(ControlError::new(
                "invalid_argument",
                "choose viewport or tail",
            ));
        }
        let mut state = crate::ghostty::RenderState::new()?;
        let snapshot = state.observe(&self.terminal)?;
        let colors = snapshot.colors()?;
        let observation = self.terminal.diagnostic_rows(
            viewport,
            (tail as usize).min(crate::control::MAX_READ_ROWS),
            crate::control::MAX_FRAME_BYTES / 2,
            &colors,
        )?;
        let rows=observation.rows.iter().map(|row| {
            let mut text=String::new();
            let cells=row.cells.iter().map(|cell| {
                let continuation=matches!(cell.width,CellWidth::SpacerTail|CellWidth::SpacerHead);
                if !continuation {text.push_str(if cell.text.is_empty(){" "}else{&cell.text});}
                CaptureCell {col:cell.column,width:cell.width.column_advance(),continuation,text:cell.text.clone(),fg:rgb_hex(cell.foreground),bg:cell.background.map(rgb_hex),bold:cell.style.bold,italic:cell.style.italic,underline:underline_name(cell.style.underline).into(),inverse:cell.style.inverse,strikethrough:cell.style.strikethrough}
            }).collect::<Vec<_>>();
            serde_json::json!({"index":row.index,"text":text,"wrapped":row.wrapped,"cells":if format == "cells"{serde_json::to_value(cells).unwrap()}else{serde_json::Value::Null}})
        }).collect::<Vec<_>>();
        let text = rows
            .iter()
            .map(|r| r["text"].as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let count = rows.len();
        let start = observation.start;
        let total = observation.total;
        Ok(
            serde_json::json!({"text":text,"rows":rows,"buffer":if alternate {"alternate"}else{"primary"},
            "output_seq":self.output_seq.to_string(),"row_start":start,"row_end":start+count,"total_rows":total,
            "source":if viewport {"active_buffer_viewport"}else{"active_buffer_tail"},"truncation":{"omitted_rows":total-count,"reason":if observation.truncated {Some("byte_limit")}else if count<total {Some("requested_range")} else {None}},"terminal_size":{"cols":self.size.0,"rows":self.size.1}}),
        )
    }
    pub fn build_feedback_capture(
        &mut self,
        offscreen: bool,
    ) -> crate::ghostty::Result<TerminalCapture> {
        // Observation must not alter the painter's reusable dirty snapshot.
        let mut state = crate::ghostty::RenderState::new()?;
        let mut row_iterator = crate::ghostty::render::RowIterator::new()?;
        let mut cell_iterator = crate::ghostty::render::CellIterator::new()?;
        let snapshot = if offscreen {
            state.observe(&self.terminal)?
        } else {
            self.render_state.current()
        };
        let colors = snapshot.colors()?;

        let mut rows = Vec::new();
        let mut row_it = row_iterator.update(&snapshot)?;
        let mut row_idx = 0u16;
        while let Some(row) = row_it.next() {
            let mut row_text = String::new();
            let mut cells = Vec::new();
            let mut cell_it = cell_iterator.update(row)?;
            let mut col_idx = 0u16;
            while let Some(cell) = cell_it.next() {
                let width = cell.width()?;
                let advance = width.column_advance();
                let text = cell.text()?;
                let continuation = matches!(width, CellWidth::SpacerTail | CellWidth::SpacerHead);
                if !continuation {
                    row_text.push_str(if text.is_empty() { " " } else { &text });
                }

                let fg = cell.fg_color()?.unwrap_or(colors.foreground);
                let bg = cell.bg_color()?;
                let style = cell.style()?;
                cells.push(CaptureCell {
                    col: col_idx,
                    width: advance,
                    continuation,
                    text,
                    fg: rgb_hex(fg),
                    bg: bg.map(rgb_hex),
                    bold: style.bold,
                    italic: style.italic,
                    underline: underline_name(style.underline).to_string(),
                    inverse: style.inverse,
                    strikethrough: style.strikethrough,
                });
                col_idx += 1;
            }

            rows.push(CaptureRow {
                index: row_idx,
                wrapped: self.terminal.viewport_row_wrapped(row_idx)?,
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
                family: self.config.font_family.clone(),
                size_px: self.config.font_size_px,
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
    }
}
