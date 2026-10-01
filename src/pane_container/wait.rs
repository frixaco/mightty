use super::*;
use crate::control::{ControlError, Request, string_arg};
use gpui::App;
use serde_json::{Value, json};

pub struct ControlWait {
    request: Request,
    terminal: Option<gpui::WeakEntity<TerminalWidget>>,
    search: Option<u64>,
    after: Option<u64>,
    pub last: Value,
}
impl ControlWait {
    pub fn probe_removed_window(&mut self) -> Result<Option<Value>, ControlError> {
        if string_arg(&self.request, "condition")? != Some("process-exited") {
            return Err(ControlError::new(
                "window_closed",
                "wait target window was closed",
            ));
        }
        self.last = crate::diagnostics::outcome(&self.request.target)
            .ok_or_else(|| ControlError::new("outcome_expired", "removed pane outcome expired"))?;
        if self.last["processes"]["availability"] == "unavailable" {
            return Err(ControlError::new(
                "unavailable",
                "root process observation unavailable",
            ));
        }
        Ok((self.last["processes"]["lifecycle"] == "exited")
            .then(|| json!({"condition":"process-exited","observation":self.last})))
    }
    pub fn release(&mut self, cx: &mut App) {
        if let (Some(terminal), Some(id)) = (
            self.terminal
                .as_ref()
                .and_then(|terminal| terminal.upgrade()),
            self.search.take(),
        ) {
            terminal.update(cx, |terminal, _| terminal.control_release_search(id));
        }
    }
}
impl PaneContainer {
    pub fn register_control_wait(
        &self,
        request: &Request,
        cx: &mut Context<Self>,
    ) -> Result<ControlWait, ControlError> {
        let condition = string_arg(request, "condition")?.unwrap_or("text");
        if !matches!(
            condition,
            "text"
                | "title-equals"
                | "layout-ready"
                | "settings-generation"
                | "overlay-open"
                | "prompt-ready"
                | "process-exited"
        ) {
            return Err(ControlError::new(
                "invalid_argument",
                "unknown wait condition",
            ));
        }
        let mut request = request.clone();
        let mut terminal = None;
        let mut search = None;
        let mut after = None;
        if matches!(condition, "text" | "prompt-ready" | "process-exited") {
            let (index, pane, entity) = self.targeted_terminal(&request, cx)?;
            request.target.tab_id = Some(format!("t{}", self.tabs[index].id.value()));
            request.target.pane_id = Some(format!("p{}", pane.value()));
            if condition == "text" {
                let text = string_arg(&request, "text")?
                    .ok_or_else(|| ControlError::new("invalid_argument", "text required"))?;
                if let Some(cursor) = string_arg(&request, "after_output")? {
                    let prefix = format!("{}:p{}:", crate::control::instance_id(), pane.value());
                    let sequence = cursor
                        .strip_prefix(&prefix)
                        .and_then(|v| v.parse::<u64>().ok())
                        .ok_or_else(|| {
                            ControlError::new(
                                "invalid_cursor",
                                "cursor belongs to another instance or pane",
                            )
                        })?;
                    let current = entity.read(cx).control_state()["output_seq"]
                        .as_str()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap();
                    if sequence > current {
                        return Err(ControlError::new(
                            "invalid_cursor",
                            "cursor is in the future",
                        ));
                    }
                    after = Some(sequence);
                }
                search =
                    Some(entity.update(cx, |terminal, _| terminal.control_register_search(text))?);
            }
            terminal = Some(entity.downgrade());
        } else if matches!(condition, "title-equals" | "layout-ready") {
            if condition == "title-equals"
                || request.target.tab_id.is_some()
                || request.target.pane_id.is_some()
                || request.target.window_id.is_none()
            {
                let index = self.control_tab(&request, cx)?;
                request.target.tab_id = Some(format!("t{}", self.tabs[index].id.value()));
            }
            if string_arg(
                &request,
                if condition == "title-equals" {
                    "value"
                } else {
                    "layout"
                },
            )?
            .is_none()
            {
                return Err(ControlError::new("invalid_argument", "wait value required"));
            }
        } else if condition == "overlay-open"
            && !matches!(string_arg(&request, "overlay")?, Some("palette" | "search"))
        {
            return Err(ControlError::new(
                "invalid_argument",
                "overlay must be palette or search",
            ));
        } else if condition == "settings-generation"
            && string_arg(&request, "at_least")?
                .and_then(|v| v.parse::<u64>().ok())
                .is_none()
        {
            return Err(ControlError::new(
                "invalid_argument",
                "at-least must be an unsigned integer",
            ));
        }
        Ok(ControlWait {
            request,
            terminal,
            search,
            after,
            last: Value::Null,
        })
    }
    pub fn probe_control_wait(
        &self,
        wait: &mut ControlWait,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Result<Option<Value>, ControlError> {
        let request = &wait.request;
        let condition = string_arg(request, "condition")?.unwrap_or("text");
        let satisfied;
        if wait.terminal.is_some() {
            if !self.control_contains(&request.target, cx) {
                let outcome = crate::diagnostics::outcome(&request.target);
                if condition == "process-exited" {
                    wait.last = outcome.ok_or_else(|| {
                        ControlError::new("outcome_expired", "removed pane outcome expired")
                    })?;
                    if wait.last["processes"]["availability"] == "unavailable" {
                        return Err(ControlError::new(
                            "unavailable",
                            "root process observation unavailable",
                        ));
                    }
                    return Ok((wait.last["processes"]["lifecycle"] == "exited")
                        .then(|| json!({"condition":condition,"observation":wait.last})));
                }
                let mut error = ControlError::new("pane_closed", "pane was removed");
                error.details = Box::new(json!({"outcome":outcome}));
                return Err(error);
            }
            let terminal = wait
                .terminal
                .as_ref()
                .and_then(|terminal| terminal.upgrade())
                .ok_or_else(|| ControlError::new("pane_closed", "pane was removed"))?;
            wait.last = terminal.read(cx).control_state();
            satisfied = match condition {
                "text" => {
                    let (matched, changed) = terminal.update(cx, |terminal, _| {
                        terminal.control_probe_search(wait.search.unwrap())
                    })?;
                    if changed {
                        return Err(ControlError::new(
                            "buffer_changed",
                            "terminal buffer changed during search",
                        ));
                    }
                    matched
                        && wait.after.is_none_or(|seq| {
                            wait.last["output_seq"]
                                .as_str()
                                .unwrap()
                                .parse::<u64>()
                                .unwrap()
                                > seq
                        })
                }
                "prompt-ready" => terminal.read(cx).control_prompt_ready()?,
                _ => {
                    if wait.last["processes"]["availability"] == "unavailable" {
                        return Err(ControlError::new(
                            "unavailable",
                            "root process observation unavailable",
                        ));
                    }
                    wait.last["processes"]["lifecycle"] == "exited"
                }
            };
        } else {
            wait.last = self.control_state(window, cx);
            satisfied = match condition {
                "settings-generation" => {
                    self.settings.generation()
                        >= string_arg(request, "at_least")?
                            .unwrap()
                            .parse::<u64>()
                            .unwrap()
                }
                "overlay-open" => {
                    if string_arg(request, "overlay")? == Some("palette") {
                        self.palette.is_some()
                    } else {
                        self.targeted_terminal(request, cx)?
                            .2
                            .read(cx)
                            .control_state()["search_open"]
                            == true
                    }
                }
                "layout-ready"
                    if request.target.tab_id.is_none() && request.target.pane_id.is_none() =>
                {
                    self.snapshot_layout_ready(request, window, cx)?
                }
                _ => {
                    let index = self.control_tab(request, cx)?;
                    wait.last = wait.last["tabs"][index].clone();
                    if condition == "title-equals" {
                        wait.last["title"] == string_arg(request, "value")?.unwrap()
                    } else {
                        if wait.last["layout_token"] != string_arg(request, "layout")?.unwrap() {
                            return Err(ControlError::new(
                                "precondition_failed",
                                "layout token superseded",
                            ));
                        }
                        wait.last["panes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|pane| pane["terminal_size"] == pane["pty_size"])
                    }
                }
            };
        }
        Ok(satisfied.then(|| json!({"condition":condition,"observation":wait.last})))
    }
}
