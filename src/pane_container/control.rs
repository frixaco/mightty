use super::*;
use crate::{
    control::{self, ControlError, Request},
    split::PaneId,
};
use serde_json::{Value, json};

impl PaneContainer {
    pub fn snapshot_layout_ready(
        &self,
        request: &Request,
        window: &Window,
        cx: &gpui::App,
    ) -> Result<bool, ControlError> {
        let index = if request.target.tab_id.is_some() {
            Some(self.control_tab(request, cx)?)
        } else {
            None
        };
        if let Some(layout) = control::string_arg(request, "layout")? {
            let mut guarded = request.clone();
            guarded.preconditions.layout_token = Some(layout.into());
            self.check_layout(&guarded, index, window, cx)?;
        }
        Ok(self
            .tabs
            .iter()
            .enumerate()
            .filter(|(i, _)| index.is_none_or(|index| index == *i))
            .all(|(_, tab)| {
                tab.split
                    .read(cx)
                    .pane_entities()
                    .all(|(_, _, terminal)| terminal.read(cx).geometry_ready())
            }))
    }
    pub fn resolve_snapshot_target(
        &self,
        request: &Request,
        cx: &gpui::App,
    ) -> Result<Request, ControlError> {
        let mut request = request.clone();
        request.target.window_id = Some(self.window_id.into());
        if request.target.pane_id.is_some() {
            let (index, pane, _) = self.targeted_terminal(&request, cx)?;
            request.target.tab_id = Some(format!("t{}", self.tabs[index].id.value()));
            request.target.pane_id = Some(format!("p{}", pane.value()));
        } else if request.target.tab_id.is_some() {
            let index = self.control_tab(&request, cx)?;
            request.target.tab_id = Some(format!("t{}", self.tabs[index].id.value()));
        }
        Ok(request)
    }
    pub fn presentation(
        &self,
        revision: u64,
        window: &Window,
        cx: &mut gpui::App,
    ) -> std::sync::Arc<crate::snapshot::Frame> {
        self.presentation_tab(self.active_tab_index, None, revision, window, cx)
    }
    fn presentation_tab(
        &self,
        index: usize,
        target: Option<PaneId>,
        revision: u64,
        window: &Window,
        cx: &mut gpui::App,
    ) -> std::sync::Arc<crate::snapshot::Frame> {
        let tab = &self.tabs[index];
        let split = tab.split.read(cx);
        let entities = split
            .pane_entities()
            .filter(|(id, _, _)| {
                target.map_or_else(
                    || split.zoomed_pane_id().is_none_or(|zoom| zoom == *id),
                    |target| target == *id,
                )
            })
            .map(|(id, profile, terminal)| (id, profile.clone(), terminal))
            .collect::<Vec<_>>();
        let mut panes = Vec::new();
        let mut cell_budget = 40000usize;
        for (id, profile, terminal) in entities {
            let count = terminal.read(cx).capture_cell_count();
            let include_source = count <= 20000 && count <= cell_budget;
            if include_source {
                cell_budget -= count;
            }
            let mut pane = terminal.update(cx, |terminal, _| {
                terminal.presentation(window, revision == 0, include_source)
            });
            pane.state["pane_id"] = json!(format!("p{}", id.value()));
            pane.state["tab_id"] = json!(format!("t{}", tab.id.value()));
            pane.state["profile_id"] = json!(profile.as_str());
            pane.state["output_cursor"] = json!(format!(
                "{}:p{}:{}",
                control::instance_id(),
                id.value(),
                pane.state["output_seq"].as_str().unwrap()
            ));
            panes.push(pane);
        }
        let mut labels = Vec::new();
        self.label_geometry
            .borrow_mut()
            .retain(|id, _| self.tabs.iter().any(|tab| tab.id.value() == *id));
        if self.sidebar_visible && index == self.active_tab_index && target.is_none() {
            for (index, tab) in self.tabs.iter().enumerate() {
                labels.push(json!({"tab_id":format!("t{}",tab.id.value()),"chosen_title":tab.title,"fallback_title":tab.default_title,"decorated_label":tab.title,"badge":if tab.bell_pending{format!("{}•",index+1)}else{(index+1).to_string()},"geometry":self.label_geometry.borrow().get(&tab.id.value()),"measured_extents":{"availability":"unavailable"}}));
            }
        }
        std::sync::Arc::new(crate::snapshot::Frame {
            schema_version: 1,
            instance_id: control::instance_id().into(),
            window_id: self.window_id.into(),
            scene_revision: revision.to_string(),
            prepared_at: crate::diagnostics::timestamp(),
            dpi_scale: window.scale_factor(),
            window: json!({"bounds":{"width":f32::from(window.viewport_size().width),"height":f32::from(window.viewport_size().height)},"layout_token":self.window_layout_token(window,cx),"active_tab_id":format!("t{}",tab.id.value()),"tab_layout_token":tab.split.read(cx).layout_token(tab.id),"content_bounds":self.content_bounds.map(crate::snapshot::rect),"sidebar_visible":self.sidebar_visible,"palette":self.palette.as_ref().map(|palette|json!({"query":palette.query,"selected":palette.selected})),"gpu":window.gpu_specs(),"settings_generation":self.settings.generation().to_string(),"settings":{"app":self.settings.current().app,"key_bindings":self.settings.current().key_bindings}}),
            panes,
            labels,
        })
    }
    pub fn offscreen(
        &self,
        request: &Request,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Result<crate::snapshot::Prepared, ControlError> {
        let (index, target) = if request.target.pane_id.is_some() {
            let (index, pane, _) = self.targeted_terminal(request, cx)?;
            (index, Some(pane))
        } else if request.target.tab_id.is_some() {
            (self.control_tab(request, cx)?, None)
        } else {
            return Err(ControlError::new(
                "unsupported",
                "offscreen requires a tab or pane target",
            ));
        };
        let bounds = if let Some(id) = target {
            self.tabs[index]
                .split
                .read(cx)
                .terminal(id)
                .unwrap()
                .read(cx)
                .control_state()["computed_bounds"]
                .clone()
        } else {
            self.content_bounds
                .map(crate::snapshot::rect)
                .ok_or_else(|| {
                    ControlError::new("layout_unavailable", "content bounds not established")
                })?
        };
        let size = gpui::size(
            px(bounds["width"].as_f64().unwrap() as f32),
            px(bounds["height"].as_f64().unwrap() as f32),
        );
        crate::snapshot::validate_size(size, window.scale_factor())?;
        let mut frame = self.presentation_tab(index, target, 0, window, cx);
        let owned = std::sync::Arc::get_mut(&mut frame).unwrap();
        owned.labels.clear();
        owned.window["palette"] = Value::Null;
        owned.window["sidebar_visible"] = json!(false);
        owned.window["image_origin_logical"] = json!({"x":bounds["x"],"y":bounds["y"]});
        owned.window["bounds"] = json!({"width":bounds["width"],"height":bounds["height"]});
        // Opening a GPUI window draws and clears the element arena; do it
        // before allocating the capture tree.
        let scratch = crate::snapshot::scratch_window(size, cx)?;
        let element = self.tabs[index]
            .split
            .update(cx, |split, cx| split.offscreen_element(target, window, cx));
        let gpu =
            crate::snapshot::paint_offscreen(scratch, element, size, window.scale_factor(), cx)?;
        Ok(crate::snapshot::Prepared {
            gpu,
            frame,
            crop: None,
            mode: "offscreen".into(),
            visibility: crate::snapshot::visibility(window),
            diagnostics: crate::diagnostics::recent(),
            diagnostics_observed_at: crate::diagnostics::timestamp(),
        })
    }
    pub(super) fn retain_pane(&self, index: usize, pane: PaneId, cx: &mut Context<Self>) {
        if let Some(terminal) = self.tabs[index].split.read(cx).terminal(pane) {
            let mut state = terminal.read(cx).control_state();
            state["window_id"] = json!(self.window_id);
            state["tab_id"] = json!(format!("t{}", self.tabs[index].id.value()));
            state["pane_id"] = json!(format!("p{}", pane.value()));
            let root = terminal.read(cx).control_root_process();
            let mut request = control::request(
                &control::Descriptor {
                    protocol_version: 1,
                    instance_id: control::instance_id().into(),
                    pid: 0,
                    process_creation_time: String::new(),
                    endpoint: String::new(),
                    started_unix_ms: "0".into(),
                },
                "pane.read",
            );
            request.args.insert("tail".into(), json!(100));
            state["final_tail"] = terminal
                .update(cx, |terminal, _| terminal.control_read(&request))
                .ok()
                .map(|read| {
                    read["text"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(8192)
                        .collect::<String>()
                })
                .map_or(Value::Null, Value::String);
            crate::diagnostics::retain_outcome(state, root);
        }
    }
    pub fn window_layout_token(&self, window: &Window, cx: &gpui::App) -> String {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        f32::from(window.viewport_size().width)
            .to_bits()
            .hash(&mut hash);
        f32::from(window.viewport_size().height)
            .to_bits()
            .hash(&mut hash);
        window.scale_factor().to_bits().hash(&mut hash);
        self.sidebar_visible.hash(&mut hash);
        self.palette.is_some().hash(&mut hash);
        self.active_tab_index.hash(&mut hash);
        for tab in &self.tabs {
            tab.id.hash(&mut hash);
            tab.title.hash(&mut hash);
            tab.split.read(cx).layout_token(tab.id).hash(&mut hash);
        }
        format!(
            "{}:window:{}:{:x}",
            control::instance_id(),
            self.window_id,
            hash.finish()
        )
    }

    pub fn check_layout(
        &self,
        request: &Request,
        index: Option<usize>,
        window: &Window,
        cx: &gpui::App,
    ) -> Result<(), ControlError> {
        if let Some(expected) = &request.preconditions.layout_token {
            let actual = index.map_or_else(
                || self.window_layout_token(window, cx),
                |i| self.tabs[i].split.read(cx).layout_token(self.tabs[i].id),
            );
            if *expected != actual {
                let mut error =
                    ControlError::new("precondition_failed", "layout changed since observation");
                error.details = Box::new(json!({"actual_layout_token":actual}));
                return Err(error);
            }
        }
        Ok(())
    }

    pub(super) fn control_tab(
        &self,
        request: &Request,
        cx: &gpui::App,
    ) -> Result<usize, ControlError> {
        if request.target.pane_id.is_some() {
            return self.control_target(&request.target, cx).map(|v| v.0);
        }
        if let Some(id) = &request.target.tab_id {
            if id == "active" {
                return Ok(self.active_tab_index);
            }
            return self
                .tabs
                .iter()
                .position(|t| format!("t{}", t.id.value()) == *id)
                .ok_or_else(|| ControlError::new("target_unavailable", "tab does not exist"));
        }
        if request.target.window_id.is_some() || self.tabs.len() == 1 {
            return Ok(self.active_tab_index);
        }
        Err(ControlError::new(
            "ambiguous_target",
            "specify tab ID or active",
        ))
    }

    fn launch_config(
        &self,
        request: &Request,
        tab: TabId,
        pane: PaneId,
    ) -> Result<(TerminalConfig, ProfileId, String), ControlError> {
        let profile = control::string_arg(request, "profile")?
            .map(|s| {
                ProfileId::new(s).map_err(|e| ControlError::new("invalid_argument", e.to_string()))
            })
            .transpose()?
            .unwrap_or_else(|| self.settings.current().default_profile.clone());
        let resolved = self.settings.current();
        let mut config = resolved
            .terminal_config(Some(&profile))
            .map_err(|e| ControlError::new("invalid_argument", e.to_string()))?;
        let label = resolved.profiles[&profile].label.clone();
        if let Some(cwd) = control::string_arg(request, "cwd")? {
            let path = PathBuf::from(cwd);
            if !path.is_absolute() || !trusted_working_directory(&path) || !path.is_dir() {
                return Err(ControlError::new(
                    "invalid_directory",
                    "cwd must be an existing absolute local directory",
                ));
            }
            config.launch.working_directory = Some(path);
        }
        if let Some(executable) = control::string_arg(request, "exec")? {
            if executable.is_empty() || executable.contains('\0') {
                return Err(ControlError::new("invalid_argument", "invalid executable"));
            }
            config.launch.executable = executable.into();
            config.launch.arguments = string_list(request, "argv")?
                .into_iter()
                .map(Into::into)
                .collect();
            config.launch.shell_integration = false;
        } else if request.args.contains_key("argv") {
            return Err(ControlError::new("invalid_argument", "argv requires exec"));
        }
        match control::string_arg(request, "env_mode")?.unwrap_or("inherit") {
            "inherit" => {}
            "empty" => {
                config.launch.inherit_environment = false;
                config.launch.environment.clear();
                config.launch.shell_integration = false;
            }
            _ => {
                return Err(ControlError::new(
                    "invalid_argument",
                    "env-mode must be inherit or empty",
                ));
            }
        }
        for name in string_list(request, "unset_env")? {
            validate_env_name(&name)?;
            config
                .launch
                .environment
                .retain(|key, _| !key.eq_ignore_ascii_case(std::ffi::OsStr::new(&name)));
            config.launch.unset_environment.insert(name.into());
        }
        for pair in string_list(request, "env")? {
            let (name, value) = pair
                .split_once('=')
                .ok_or_else(|| ControlError::new("invalid_argument", "env requires NAME=VALUE"))?;
            validate_env_name(name)?;
            if value.contains('\0') {
                return Err(ControlError::new(
                    "invalid_argument",
                    "environment value contains NUL",
                ));
            }
            config
                .launch
                .environment
                .retain(|key, _| !key.eq_ignore_ascii_case(std::ffi::OsStr::new(name)));
            config.launch.environment.insert(name.into(), value.into());
        }
        Self::identify_launch(&mut config, self.window_id, tab, pane);
        Ok((config, profile, label))
    }

    pub(super) fn targeted_terminal(
        &self,
        request: &Request,
        cx: &gpui::App,
    ) -> Result<(usize, PaneId, Entity<TerminalWidget>), ControlError> {
        let mut target = request.target.clone();
        if target.pane_id.is_none() && target.tab_id.is_none() && target.window_id.is_some() {
            target.tab_id = Some("active".into());
        }
        self.control_target(&target, cx)
    }

    fn tab_result(&self, index: usize, window: &Window, cx: &gpui::App) -> Value {
        let mut result = self.control_state(window, cx)["tabs"][index].clone();
        result["layout_token"] = json!(
            self.tabs[index]
                .split
                .read(cx)
                .layout_token(self.tabs[index].id)
        );
        result
    }

    pub(super) fn close_tab_target(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Value {
        let removed = format!("t{}", self.tabs[index].id.value());
        let panes = self.tabs[index]
            .split
            .read(cx)
            .pane_entities()
            .map(|(id, _, _)| id)
            .collect::<Vec<_>>();
        for pane in panes {
            self.retain_pane(index, pane, cx);
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            window.remove_window();
        } else {
            if self.active_tab_index == index {
                self.active_tab_index = index.saturating_sub(1).min(self.tabs.len() - 1);
                self.needs_focus = true;
            } else if self.active_tab_index > index {
                self.active_tab_index -= 1;
            }
            self.establish_layout(window, cx);
        }
        cx.notify();
        json!({"removed_tab_id":removed,"active_tab_id":self.tabs.get(self.active_tab_index).map(|t|format!("t{}",t.id.value())),"window_closed":self.tabs.is_empty()})
    }

    pub(super) fn control_mutation(
        &mut self,
        request: &Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, ControlError> {
        match request.op.as_str() {
            "ui.sidebar" => {
                self.check_layout(request, None, window, cx)?;
                self.sidebar_visible = control::bool_arg(request, "visible", self.sidebar_visible)?;
                self.establish_layout(window, cx);
                cx.notify();
                Ok(self.control_state(window, cx))
            }
            "ui.palette" => {
                self.check_layout(request, None, window, cx)?;
                let open = control::bool_arg(request, "open", self.palette.is_some())?;
                let query = control::string_arg(request, "query")?;
                if query.is_some_and(|query| query.len() > 4096) {
                    return Err(ControlError::new(
                        "invalid_argument",
                        "query exceeds 4096 bytes",
                    ));
                }
                if !open && query.is_some() {
                    return Err(ControlError::new(
                        "invalid_argument",
                        "query requires an open palette",
                    ));
                }
                if open {
                    if self.palette.is_none() {
                        self.open_palette(window, cx);
                    }
                    if let Some(query) = query {
                        if let Some(palette) = &mut self.palette {
                            palette.query = query.into();
                            palette.selected = 0;
                        }
                        if let Some(input) = &self.palette_input {
                            input.update(cx, |input, cx| {
                                input.set_value(query.to_string(), window, cx)
                            });
                        }
                    }
                } else {
                    self.close_palette(window, cx);
                }
                cx.notify();
                Ok(self.control_state(window, cx))
            }
            "ui.search" => {
                let (index, _, terminal) = self.targeted_terminal(request, cx)?;
                self.check_layout(request, Some(index), window, cx)?;
                let open = control::bool_arg(
                    request,
                    "open",
                    terminal.read(cx).control_state()["search_open"] == true,
                )?;
                let query = control::string_arg(request, "query")?;
                if query.is_some_and(|query| query.len() > 4096) || (!open && query.is_some()) {
                    return Err(ControlError::new(
                        "invalid_argument",
                        "query requires open search and at most 4096 bytes",
                    ));
                }
                let visible = index == self.active_tab_index;
                terminal.update(cx, |terminal, cx| {
                    terminal.control_search(open, query, visible, window, cx)
                });
                Ok(self.tab_result(index, window, cx))
            }
            "tab.new" => {
                self.check_layout(request, None, window, cx)?;
                if self.tabs.len() >= MAX_SELECTABLE_TABS {
                    return Err(ControlError::new("tab_limit", "at most nine tabs"));
                }
                let focus = control::bool_arg(request, "focus", false)?;
                let tab_id = TabId::fresh();
                let pane_id = PaneId::allocate();
                let (config, profile, label) = self.launch_config(request, tab_id, pane_id)?;
                let terminal = Self::create_terminal(config, self.exit_tx.clone(), cx);
                if !terminal.read(cx).launch_succeeded() {
                    return Err(ControlError::new("launch_failed", "shell launch failed"));
                }
                let split = cx.new(|_| Split::with_terminal_id(pane_id, terminal, profile));
                self.tabs.push(Tab {
                    id: tab_id,
                    split,
                    title: label.clone(),
                    default_title: label,
                    bell_pending: false,
                });
                let index = self.tabs.len() - 1;
                if focus {
                    self.active_tab_index = index;
                    self.needs_focus = true;
                }
                self.establish_layout(window, cx);
                cx.notify();
                Ok(self.tab_result(index, window, cx))
            }
            "tab.select" | "tab.move" | "tab.close" => {
                let index = self.control_tab(request, cx)?;
                self.check_layout(request, None, window, cx)?;
                match request.op.as_str() {
                    "tab.select" => self.activate_tab(index, cx),
                    "tab.close" => return Ok(self.close_tab_target(index, window, cx)),
                    _ => {
                        let before = control::string_arg(request, "before")?.ok_or_else(|| {
                            ControlError::new("invalid_argument", "before tab ID required")
                        })?;
                        let target = self
                            .tabs
                            .iter()
                            .position(|t| format!("t{}", t.id.value()) == before)
                            .ok_or_else(|| {
                                ControlError::new("target_unavailable", "before tab does not exist")
                            })?;
                        if target != index {
                            let selected = self.tabs[self.active_tab_index].id;
                            let tab = self.tabs.remove(index);
                            self.tabs
                                .insert(if target > index { target - 1 } else { target }, tab);
                            self.active_tab_index =
                                self.tabs.iter().position(|t| t.id == selected).unwrap();
                            cx.notify();
                        }
                    }
                }
                Ok(self.control_state(window, cx))
            }
            "window.resize" => {
                self.check_layout(request, None, window, cx)?;
                let width = control::number_arg(request, "width", 0.0)?;
                let height = control::number_arg(request, "height", 0.0)?;
                if !(100.0..=16384.0).contains(&width) || !(100.0..=16384.0).contains(&height) {
                    return Err(ControlError::new(
                        "invalid_argument",
                        "client dimensions must be 100..16384 logical pixels",
                    ));
                }
                window.resize(gpui::size(px(width as f32), px(height as f32)));
                self.establish_layout(window, cx);
                cx.notify();
                Ok(self.control_state(window, cx))
            }
            "window.focus" => {
                #[cfg(windows)]
                crate::application::windows::show_default_terminal_window(window)
                    .map_err(|e| ControlError::new("platform_error", e.to_string()))?;
                window.activate_window();
                Ok(json!({"os_focused":window.is_window_active()}))
            }
            "pane.split" | "pane.resize" | "pane.zoom" | "pane.close" | "pane.focus"
            | "pane.scroll" | "pane.send-text" | "pane.send-key" | "pane.input" => {
                let (index, pane_id, terminal) = self.targeted_terminal(request, cx)?;
                self.check_layout(request, Some(index), window, cx)?;
                let split = self.tabs[index].split.clone();
                match request.op.as_str() {
                    "pane.split" => {
                        if self
                            .tabs
                            .iter()
                            .map(|tab| tab.split.read(cx).pane_count())
                            .sum::<usize>()
                            >= 128
                        {
                            return Err(ControlError::new(
                                "pane_limit",
                                "at most 128 panes per window",
                            ));
                        }
                        let direction = control::direction(
                            control::string_arg(request, "direction")?.ok_or_else(|| {
                                ControlError::new("invalid_argument", "direction required")
                            })?,
                        )?;
                        let ratio = control::number_arg(request, "ratio", 0.5)?;
                        if !(0.0..1.0).contains(&ratio) || ratio == 0.0 {
                            return Err(ControlError::new(
                                "invalid_argument",
                                "ratio must be between zero and one",
                            ));
                        }
                        let focus = control::bool_arg(request, "focus", false)?;
                        let new_id = PaneId::allocate();
                        let (config, profile, _) =
                            self.launch_config(request, self.tabs[index].id, new_id)?;
                        let new_terminal = Self::create_terminal(config, self.exit_tx.clone(), cx);
                        if !new_terminal.read(cx).launch_succeeded() {
                            return Err(ControlError::new("launch_failed", "shell launch failed"));
                        }
                        split.update(cx, |s, _| {
                            s.split_target(
                                pane_id,
                                new_id,
                                direction,
                                ratio as f32,
                                (new_terminal, profile),
                                focus,
                            )
                        });
                        if focus {
                            self.active_tab_index = index;
                            self.needs_focus = true;
                        }
                        self.establish_layout(window, cx);
                        cx.notify();
                        let mut result = self.tab_result(index, window, cx);
                        result["new_pane_id"] = json!(format!("p{}", new_id.value()));
                        Ok(result)
                    }
                    "pane.resize" => {
                        let direction =
                            control::direction(control::string_arg(request, "edge")?.ok_or_else(
                                || ControlError::new("invalid_argument", "edge required"),
                            )?)?;
                        let delta = control::number_arg(request, "delta_px", 0.0)?;
                        if delta.abs() > 16384.0 {
                            return Err(ControlError::new(
                                "invalid_argument",
                                "delta exceeds 16384 logical pixels",
                            ));
                        }
                        if !split.update(cx, |s, _| {
                            s.resize_target(pane_id, direction, delta as f32, window)
                        }) {
                            return Err(ControlError::new(
                                "edge_unavailable",
                                "pane has no divider on that edge",
                            ));
                        }
                        self.establish_layout(window, cx);
                        cx.notify();
                        Ok(self.tab_result(index, window, cx))
                    }
                    "pane.zoom" => {
                        let enabled = control::bool_arg(request, "enabled", false)?;
                        split.update(cx, |s, _| s.zoom_target(pane_id, enabled));
                        self.establish_layout(window, cx);
                        cx.notify();
                        Ok(self.tab_result(index, window, cx))
                    }
                    "pane.focus" => {
                        let direction = control::string_arg(request, "direction")?
                            .map(control::direction)
                            .transpose()?;
                        if !split.update(cx, |s, cx| s.focus_target(pane_id, direction, window, cx))
                        {
                            return Err(ControlError::new(
                                "neighbor_unavailable",
                                "no pane in that direction",
                            ));
                        }
                        self.active_tab_index = index;
                        self.needs_focus = true;
                        self.establish_layout(window, cx);
                        cx.notify();
                        Ok(self.tab_result(index, window, cx))
                    }
                    "pane.close" => {
                        if split.read(cx).pane_count() == 1 {
                            return Ok(self.close_tab_target(index, window, cx));
                        }
                        let selected = split.read(cx).active_pane_id() == pane_id;
                        self.retain_pane(index, pane_id, cx);
                        split.update(cx, |s, _| s.remove_pane(pane_id));
                        if selected && index == self.active_tab_index {
                            self.needs_focus = true;
                        }
                        self.establish_layout(window, cx);
                        cx.notify();
                        let mut result = self.tab_result(index, window, cx);
                        result["removed_pane_id"] = json!(format!("p{}", pane_id.value()));
                        Ok(result)
                    }
                    "pane.scroll" => terminal.update(cx, |t, cx| t.control_scroll(request, cx)),
                    _ => {
                        let (mut result, ack) =
                            terminal.update(cx, |t, _| t.control_input(request))?;
                        self.control_acks.push(ack);
                        result["output_cursor"] = json!(format!(
                            "{}:p{}:{}",
                            control::instance_id(),
                            pane_id.value(),
                            result["output_seq"].as_str().unwrap()
                        ));
                        result["pane_id"] = json!(format!("p{}", pane_id.value()));
                        result["tab_id"] = json!(format!("t{}", self.tabs[index].id.value()));
                        Ok(result)
                    }
                }
            }
            _ => Err(ControlError::new(
                "unsupported_operation",
                "unsupported operation",
            )),
        }
    }
}

fn string_list(request: &Request, name: &str) -> Result<Vec<String>, ControlError> {
    request.args.get(name).map_or(Ok(Vec::new()), |v| {
        serde_json::from_value(v.clone()).map_err(|_| {
            ControlError::new("invalid_argument", format!("{name} must be a text array"))
        })
    })
}
fn validate_env_name(name: &str) -> Result<(), ControlError> {
    if name.is_empty()
        || name.contains(['\0', '='])
        || name.to_ascii_uppercase().starts_with("MIGHTTY_")
    {
        return Err(ControlError::new(
            "invalid_argument",
            "invalid or reserved environment name",
        ));
    }
    Ok(())
}

pub(super) fn topology(node: &crate::split::SplitNode) -> Value {
    use crate::split::SplitNode;
    match node {
        SplitNode::Leaf { pane_id } => {
            json!({"type":"leaf","pane_id":format!("p{}",pane_id.value())})
        }
        SplitNode::Branch {
            axis,
            ratio,
            first,
            second,
        } => {
            json!({"type":"branch","axis":axis,"ratio":ratio,"first":topology(first),"second":topology(second)})
        }
    }
}
