//! Targeted terminal control bypasses application overlays and shortcuts.
use super::{TerminalWidget, input, pty::PtyCommand};
use crate::control::{self, ControlError, InputStep, KeyEvent, Request};
use crate::ghostty::{ViewportScroll, key::Action};
use serde_json::{Value, json};

impl TerminalWidget {
    pub fn control_search(
        &mut self,
        open: bool,
        query: Option<&str>,
        visible: bool,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if open {
            self.prepare_search(visible, window, cx);
            if let Some(query) = query {
                self.search.as_mut().unwrap().query = query.into();
                if let Some(input) = &self.search_input {
                    input.update(cx, |input, cx| {
                        input.set_value(query.to_string(), window, cx)
                    });
                }
                self.restart_search(cx);
            }
        } else if visible {
            self.close_search(window, cx);
        } else {
            self.terminal.stop_search();
            self.search = None;
            self.search_input = None;
            self.search_input_subscription = None;
            self.search_task = gpui::Task::ready(());
            cx.notify();
        }
    }
    pub fn control_input(
        &mut self,
        request: &Request,
    ) -> Result<(Value, flume::Receiver<control::Acknowledgement>), ControlError> {
        let steps=match request.op.as_str() {
            "pane.send-text"=>vec![InputStep::Text{text:control::string_arg(request,"text")?.ok_or_else(||ControlError::new("invalid_argument","text file required"))?.to_string()}],
            "pane.send-key"=>serde_json::from_value::<InputStep>(json!({"type":"key","key":control::string_arg(request,"key")?.ok_or_else(||ControlError::new("invalid_argument","key required"))?,"modifiers":request.args.get("modifiers").cloned().unwrap_or(json!([])),"event":request.args.get("event").cloned().unwrap_or(json!("tap"))})).map(|step|vec![step]).map_err(|e|ControlError::new("invalid_argument",e.to_string()))?,
            _=>serde_json::from_value::<Vec<InputStep>>(request.args.get("steps").cloned().ok_or_else(||ControlError::new("invalid_argument","input steps required"))?).map_err(|e|ControlError::new("invalid_argument",e.to_string()))?,
        };
        if steps.is_empty() || steps.len() > 1024 {
            return Err(ControlError::new(
                "invalid_argument",
                "input requires 1..1024 steps",
            ));
        }
        let source_bytes = steps
            .iter()
            .map(|step| match step {
                InputStep::Text { text } => text.len(),
                InputStep::Key { key, modifiers, .. } => {
                    key.len() + modifiers.iter().map(String::len).sum::<usize>()
                }
            })
            .sum::<usize>();
        if source_bytes > control::MAX_INPUT_BYTES {
            return Err(ControlError::new(
                "input_limit",
                "source input exceeds 256 KiB",
            ));
        }
        let mut held = std::collections::BTreeSet::new();
        let mut encoded = Vec::new();
        let mut ends = Vec::new();
        for step in &steps {
            match step {
                InputStep::Text { text } => {
                    if !crate::ghostty::paste::is_safe(text.as_bytes()) {
                        return Err(ControlError::new(
                            "paste_confirmation_required",
                            "terminal paste policy requires confirmation for this text",
                        ));
                    }
                    encoded.extend(self.terminal.encode_paste(text.as_bytes())?);
                }
                InputStep::Key {
                    key,
                    modifiers,
                    event,
                } => {
                    let stroke = control::keystroke(key, modifiers)?;
                    let identity = input::key_identity(&stroke);
                    let actions: &[Action] = match event {
                        KeyEvent::Tap => {
                            if held.contains(&identity) {
                                return Err(ControlError::new(
                                    "invalid_argument",
                                    "tap of already held key",
                                ));
                            }
                            &[Action::Press, Action::Release]
                        }
                        KeyEvent::Press => {
                            if !held.insert(identity) {
                                return Err(ControlError::new(
                                    "invalid_argument",
                                    "duplicate key press",
                                ));
                            }
                            &[Action::Press]
                        }
                        KeyEvent::Repeat => {
                            if !held.contains(&identity) {
                                return Err(ControlError::new(
                                    "invalid_argument",
                                    "repeat of unheld key",
                                ));
                            }
                            &[Action::Repeat]
                        }
                        KeyEvent::Release => {
                            if !held.remove(&identity) {
                                return Err(ControlError::new(
                                    "invalid_argument",
                                    "release of unheld key",
                                ));
                            }
                            &[Action::Release]
                        }
                    };
                    for action in actions {
                        if let Some(bytes) = input::encode_key_event_checked(
                            &mut self.key_encoder,
                            &mut self.key_event,
                            &self.terminal,
                            *action,
                            &stroke,
                        )? {
                            encoded.extend(bytes);
                        }
                    }
                }
            }
            if encoded.len() > control::MAX_INPUT_BYTES {
                return Err(ControlError::new(
                    "input_limit",
                    "encoded input exceeds byte limit",
                ));
            }
            ends.push(encoded.len());
        }
        if !held.is_empty() {
            return Err(ControlError::new(
                "invalid_argument",
                "input sequence leaves keys held",
            ));
        }
        let tx = self
            .pty_tx
            .as_ref()
            .ok_or_else(|| ControlError::new("pty_unavailable", "terminal has no running PTY"))?;
        let (reply, ack) = flume::bounded(1);
        let encoded_bytes = encoded.len();
        let baseline = self.output_seq;
        tx.send(PtyCommand::WriteAck(encoded, reply))
            .map_err(|e| ControlError::new("busy", e))?;
        Ok((
            json!({"output_seq":baseline.to_string(),"output_cursor":baseline.to_string(),"encoded_bytes":encoded_bytes,"step_byte_ends":ends}),
            ack,
        ))
    }

    pub fn control_scroll(
        &mut self,
        request: &Request,
        cx: &mut gpui::Context<Self>,
    ) -> Result<Value, ControlError> {
        if let Some(to) = control::string_arg(request, "to")? {
            if request.args.contains_key("rows") {
                return Err(ControlError::new("invalid_argument", "choose rows or to"));
            }
            self.terminal.scroll_viewport(match to {
                "top" => ViewportScroll::Top,
                "bottom" => ViewportScroll::Bottom,
                _ => {
                    return Err(ControlError::new(
                        "invalid_argument",
                        "to must be top or bottom",
                    ));
                }
            });
        } else {
            let rows = control::number_arg(request, "rows", 0.0)?;
            if rows.fract() != 0.0 || rows.abs() > 1_000_000.0 {
                return Err(ControlError::new(
                    "invalid_argument",
                    "rows must be an integer within +/-1000000",
                ));
            }
            self.terminal
                .scroll_viewport(ViewportScroll::Delta(rows as isize));
        }
        cx.notify();
        let s = self.terminal.scrollbar()?;
        Ok(json!({"offset":s.offset,"length":s.len,"total":s.total}))
    }
}
