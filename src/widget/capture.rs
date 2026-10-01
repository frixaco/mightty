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
    pub fn capture_cell_count(&self, offscreen: bool) -> usize {
        let size = self
            .painted_terminal(offscreen)
            .as_ref()
            .map(|painted| painted.frame.size)
            .or_else(|| self.committed.as_ref().map(|frame| frame.size))
            .unwrap_or(self.size);
        usize::from(size.0) * usize::from(size.1)
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
        let frame = self
            .painted_terminal(offscreen)
            .as_ref()
            .map(|painted| painted.frame.clone())
            .or_else(|| self.committed.clone());
        let key = frame.as_ref().map_or(0, |frame| frame.revision).to_string();
        let source = if !include_source || self.capture_cell_count(offscreen) > 20000 {
            self.capture_cache = None;
            None
        } else {
            if self
                .capture_cache
                .as_ref()
                .is_none_or(|(old, _)| *old != key)
            {
                self.capture_cache = frame
                    .as_ref()
                    .map(|frame| (key, std::sync::Arc::new(feedback_capture(frame))));
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
        let painted = self.painted_terminal(offscreen);
        state["focus_appearance"] = serde_json::json!(painted.map_or_else(
            || self.focus_handle.is_focused(window),
            |painted| painted.focused
        ));
        state["cursor_phase"] = serde_json::json!(
            painted.map_or(self.cursor_blink_phase, |painted| painted.cursor_phase)
        );
        if let Some(frame) = &frame {
            state["output_seq"] = serde_json::json!(frame.output_seq.to_string());
            state["output_cursor"] = serde_json::json!(format!(
                "{}:{}",
                crate::control::instance_id(),
                frame.output_seq
            ));
            state["terminal_size"] = serde_json::json!({"cols":frame.size.0,"rows":frame.size.1});
            state["viewport"] = serde_json::json!({"offset":frame.scrollbar.offset,"length":frame.scrollbar.len,"total":frame.scrollbar.total});
            state["terminal_status"] = serde_json::json!({"source":"committed_presentation","value":{"cursor_visible":frame.cursor.visible,"cursor":frame.cursor.position.map(|p| (p.x,p.y)),"cursor_footprint":frame.cursor_footprint(),"cursor_shape":format!("{:?}",frame.cursor.shape),"cursor_blinking":frame.cursor.blinking,"alternate_buffer":frame.alternate}});
            state["selection"] = serde_json::json!(frame.selection.is_some());
            state["presentation_revision"] = serde_json::json!(frame.revision.to_string());
            state["font"] =
                serde_json::json!({"family":frame.font_family,"size_px":frame.font_size_px});
        }
        state["preedit"] = serde_json::json!(
            painted.map_or(self.preedit.as_str(), |painted| painted.preedit.as_str())
        );
        state["composing"] =
            serde_json::json!(painted.map_or(self.composing, |painted| painted.composing));
        state["pending_paste_confirmation"] = serde_json::json!(self.pending_paste.is_some());
        state["selection_coordinates"] =
            serde_json::json!(frame.as_ref().and_then(|frame| frame.selection));
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
        let family = frame
            .as_ref()
            .map_or(self.config.font_family.as_str(), |frame| {
                frame.font_family.as_str()
            });
        state["painter"] = serde_json::json!({"font_features":format!("{:?}",super::render::terminal_font_features()),"fallback_chain":format!("{:?}",super::render::terminal_font_fallbacks(family)),"resolved_font_faces":{"availability":"unavailable","reason":"GPUI does not retain face/cluster diagnostics for this surface"}});
        crate::snapshot::PaneFrame { state, source }
    }
    fn painted_terminal(&self, offscreen: bool) -> Option<&super::presentation::PaintedTerminal> {
        if offscreen {
            self.offscreen_painted.as_ref()
        } else {
            self.painted.as_ref()
        }
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
        let filtered = self
            .reported_title
            .as_deref()
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .take(128)
            .collect::<String>();
        let status = self.terminal.diagnostic_status().map_or_else(
            |error| json!({"availability":"unavailable","reason":error.to_string()}),
            |status| json!({"availability":"observed","source":"live_terminal","value":status}),
        );
        json!({"title":{"reported":self.reported_title,"normalized":crate::shell_integration::display_title(self.reported_title.as_deref()),"observed_at":self.title_observed_at,"source":"ghostty_osc_title","source_limit_bytes":2047,"normalization":{"removed_controls":self.reported_title.as_ref().is_some_and(|title|title.chars().any(char::is_control)),"limited_to_128_scalars":self.reported_title.as_ref().is_some_and(|title|title.chars().filter(|c|!c.is_control()).count()>128),"trimmed":filtered.trim()!=filtered}},
            "working_directory":{"configured":self.config.launch.working_directory,"reported":self.reported_working_directory,"observed_at":self.directory_observed_at,"resolved_local":self.workspace_working_directory(),"source":if self.reported_working_directory.is_none(){"configured"}else if self.reported_local_working_directory().is_some(){"shell_local"}else{"shell_remote_untrusted_or_absent"}},
            "launch":{"executable":self.config.launch.executable,"argv":self.config.launch.arguments.iter().map(|v|v.to_string_lossy()).collect::<Vec<_>>(),
                "settings_generation":self.config.settings_generation.map(|v|v.to_string()),"inherit_environment":self.config.launch.inherit_environment,"unset_environment":self.config.launch.unset_environment.iter().map(|v|v.to_string_lossy()).collect::<Vec<_>>(),"shell_integration":self.config.launch.shell_integration,"environment_values":{"availability":"omitted","reason":"live metadata exposes configured names only"},"environment_names":self.config.launch.environment.keys().map(|v|v.to_string_lossy()).collect::<Vec<_>>()},
            "effective_settings":{"font_family":self.config.font_family,"font_size_px":self.config.font_size_px,"cursor_style":self.config.cursor_style,"cursor_blink":self.config.cursor_blink,"blink_interval_ms":self.config.blink_interval.as_millis().to_string(),"scrollback":self.config.scrollback,"theme":{"foreground":u32::from(self.theme.foreground),"background":u32::from(self.theme.background),"cursor":u32::from(self.theme.cursor),"selection":u32::from(self.theme.selection),"palette":self.theme.palette.map(u32::from)},"clipboard_policy":self.config.terminal_clipboard_policy,"action_bindings":self.config.action_bindings,"live_applied_fields":["action_bindings"],"bindings_generation":self.bindings_generation.map(|v|v.to_string())},
            "terminal_status":status,"integration":{"availability":if self.semantic_commands_available{"observed"}else{"unavailable"},"prompt_phase":if self.semantic_commands_available{self.terminal.cursor_at_prompt().ok()}else{None}},
            "terminal_size":{"cols":self.size.0,"rows":self.size.1},"computed_bounds":bounds,
            "pty_size":self.pty_tx.as_ref().and_then(|tx|tx.acknowledged_size()).map(|s|json!({"cols":s.cols,"rows":s.rows})),
            "output_seq":self.output_seq.to_string(),"output_cursor":format!("{}:{}",crate::control::instance_id(),self.output_seq),
            "lifecycle":if self.has_exited {"output_ended"} else {"running"},"output_eof":self.output_eof,"io_error":self.io_error,"processes":self.pty_worker.as_ref().and_then(|worker|worker.root_process()).map(|root|root.state()).unwrap_or(serde_json::json!({"availability":"unavailable"})),
            "font":{"family":self.config.font_family,"size_px":self.config.font_size_px},
            "viewport":self.terminal.scrollbar().ok().map(|s|json!({"offset":s.offset,"length":s.len,"total":s.total})),
            "selection":self.terminal.has_selection().ok(),"selection_coordinates":self.terminal.selection_coordinates().ok().flatten(),"search_open":self.search.is_some(),"search":self.search.as_ref().map(|search|json!({"query":search.query,"progress":format!("{:?}",search.progress),"matches":search.ranges.len(),"diagnostic":search.diagnostic}))})
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
            "source":if viewport {"active_buffer_viewport"}else{"active_buffer_tail"},"state_source":"live_terminal","truncation":{"omitted_rows":total-count,"reason":if observation.truncated {Some("byte_limit")}else if count<total {Some("requested_range")} else {None}},"terminal_size":{"cols":self.size.0,"rows":self.size.1}}),
        )
    }
    pub fn build_feedback_capture(
        &mut self,
        _offscreen: bool,
    ) -> crate::ghostty::Result<TerminalCapture> {
        let frame = self
            .committed
            .as_ref()
            .ok_or(crate::ghostty::Error::InvalidValue)?;
        Ok(feedback_capture(frame))
    }
}

pub(super) fn feedback_capture(frame: &super::presentation::TerminalFrame) -> TerminalCapture {
    let colors = &frame.colors;
    let rows = frame
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut row_text = String::new();
            let cells = row
                .cells
                .iter()
                .enumerate()
                .map(|(column, cell)| {
                    let advance = cell.width.column_advance();
                    let continuation =
                        matches!(cell.width, CellWidth::SpacerTail | CellWidth::SpacerHead);
                    if !continuation {
                        row_text.push_str(if cell.text.is_empty() {
                            " "
                        } else {
                            &cell.text
                        });
                    }
                    CaptureCell {
                        col: column as u16,
                        width: advance,
                        continuation,
                        text: cell.text.clone(),
                        fg: rgb_hex(cell.foreground.unwrap_or(colors.foreground)),
                        bg: cell.background.map(rgb_hex),
                        bold: cell.style.bold,
                        italic: cell.style.italic,
                        underline: underline_name(cell.style.underline).into(),
                        inverse: cell.style.inverse,
                        strikethrough: cell.style.strikethrough,
                    }
                })
                .collect();
            CaptureRow {
                index: index as u16,
                wrapped: row.wrapped,
                text: row_text,
                cells,
            }
        })
        .collect();
    TerminalCapture {
        captured_unix_ms: feedback::unix_timestamp_ms(),
        terminal_size: GridSize {
            cols: frame.size.0,
            rows: frame.size.1,
        },
        cell_size_px: SizePx {
            width: frame.cell_size.0.into(),
            height: frame.cell_size.1.into(),
        },
        font: FontCapture {
            family: frame.font_family.clone(),
            size_px: frame.font_size_px,
        },
        colors: CaptureColors {
            foreground: rgb_hex(colors.foreground),
            background: rgb_hex(colors.background),
            cursor: colors.cursor.map(rgb_hex),
        },
        cursor: frame
            .cursor
            .position
            .filter(|_| frame.cursor.visible)
            .map(|cursor| CaptureCursor {
                x: cursor.x,
                y: cursor.y,
            }),
        rows,
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
