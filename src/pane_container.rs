use gpui::{
    AnyElement, Context, Entity, FocusHandle, Font, FontFallbacks, IntoElement, KeyDownEvent,
    KeyUpEvent, MouseButton, MouseDownEvent, Render, Task, Timer, Window, WindowControlArea, div,
    font, prelude::*, px,
};
use gpui_component::InteractiveElementExt;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use crate::action::{
    ActionContext, AppAction, DispatchAppAction, SplitDirection as ActionSplitDirection,
};
use crate::command_palette::{PaletteCommand, commands, filtered_command_indices};
use crate::profile::ProfileId;
use crate::settings::{ReloadOutcome, SettingsStore};
#[cfg(windows)]
use crate::shell::PtyParts;
use crate::split::{Split, SplitAxis};
use crate::widget::{TerminalConfig, TerminalEvent, TerminalWidget};
use crate::workspace::{
    TabId, WorkspaceId, WorkspaceLayout, WorkspacePane, WorkspaceStore, WorkspaceTab,
    trusted_working_directory,
};

const WINDOW_BACKGROUND: u32 = 0x000000;
const WINDOW_HORIZONTAL_PADDING_PX: f32 = 8.0;
const TITLE_BAR_HEIGHT_PX: f32 = 34.0;
const MAC_TRAFFIC_LIGHT_SPACER_PX: f32 = 78.0;
const WINDOW_CONTROL_WIDTH_PX: f32 = 46.0;
const SIDEBAR_WIDTH_PX: f32 = 160.0;
const SIDEBAR_GAP_PX: f32 = 8.0;
const TAB_RADIUS_PX: f32 = 4.0;
const TAB_HEIGHT_PX: f32 = 42.0;
const MAX_SELECTABLE_TABS: usize = 9;
const PALETTE_MAX_RESULTS: usize = 12;

#[derive(Clone, Copy)]
enum WindowsCaptionButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

struct Tab {
    id: TabId,
    split: Entity<Split>,
    title: String,
    default_title: String,
    bell_pending: bool,
}

struct PaletteState {
    query: String,
    selected: usize,
}

pub struct PaneContainer {
    tabs: Vec<Tab>,
    active_tab_index: usize,
    sidebar_visible: bool,
    needs_focus: bool,
    settings: SettingsStore,
    settings_task: Task<()>,
    workspace_store: WorkspaceStore,
    workspace_ids: Vec<WorkspaceId>,
    workspace_diagnostic: Option<String>,
    palette: Option<PaletteState>,
    palette_focus: FocusHandle,
    exit_tx: flume::Sender<()>,
    exit_task: Task<()>,
}

impl PaneContainer {
    pub fn new(settings: SettingsStore, cx: &mut Context<Self>) -> Self {
        let (config, profile_id, title, sidebar_visible) = {
            let resolved = settings.current();
            let config = resolved
                .terminal_config(None)
                .expect("resolved settings contain their default profile");
            let profile_id = resolved.default_profile.clone();
            let title = resolved
                .profiles
                .get(&profile_id)
                .expect("resolved settings contain their default profile")
                .label
                .clone();
            (config, profile_id, title, resolved.app.sidebar_visible)
        };
        let (exit_tx, exit_rx) = flume::unbounded();
        let tab = Self::create_tab(config, profile_id, title, exit_tx.clone(), cx);

        let workspace_store = WorkspaceStore::open_default();
        let workspace_ids = workspace_store.list().unwrap_or_default();
        let mut container = Self {
            tabs: vec![tab],
            active_tab_index: 0,
            sidebar_visible,
            needs_focus: true,
            settings,
            settings_task: Task::ready(()),
            workspace_store,
            workspace_ids,
            workspace_diagnostic: None,
            palette: None,
            palette_focus: cx.focus_handle(),
            exit_tx,
            exit_task: Task::ready(()),
        };
        container.start_exit_task(exit_rx, cx);
        container.start_settings_task(cx);
        container
    }

    fn create_tab(
        config: TerminalConfig,
        profile_id: ProfileId,
        title: String,
        exit_tx: flume::Sender<()>,
        cx: &mut Context<Self>,
    ) -> Tab {
        let terminal = Self::create_terminal(config, exit_tx, cx);
        let split = cx.new(|_cx| Split::with_terminal(terminal, profile_id));

        Tab {
            id: TabId::fresh(),
            split,
            title: title.clone(),
            default_title: title,
            bell_pending: false,
        }
    }

    fn create_terminal(
        config: TerminalConfig,
        exit_tx: flume::Sender<()>,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalWidget> {
        let terminal = cx.new(|cx| TerminalWidget::new(config, cx));
        terminal.update(cx, |terminal, _cx| terminal.set_exit_signal(exit_tx));
        cx.subscribe(&terminal, Self::on_terminal_event).detach();
        terminal
    }

    /// Open a Windows terminal handoff as an active tab.
    ///
    /// The first handoff can replace the one tab created for a hidden COM start.
    /// Later handoffs add a tab without removing user sessions.
    #[cfg(windows)]
    pub fn open_handoff(
        &mut self,
        parts: PtyParts,
        startup_title: Option<String>,
        replace_existing: bool,
        cx: &mut Context<Self>,
    ) {
        let replace_initial_tab = replace_existing && self.tabs.len() == 1;
        if !replace_initial_tab && self.tabs.len() >= MAX_SELECTABLE_TABS {
            return;
        }

        let (config, profile_id, profile_title) = {
            let settings = self.settings.current();
            let profile_id = settings.default_profile.clone();
            let config = settings
                .terminal_config(Some(&profile_id))
                .expect("resolved settings contain their default profile");
            let profile_title = settings
                .profiles
                .get(&profile_id)
                .expect("resolved settings contain their default profile")
                .label
                .clone();
            (config, profile_id, profile_title)
        };
        let title = startup_title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(profile_title);
        let terminal = cx.new(|cx| TerminalWidget::from_pty_parts(config, parts, cx));
        terminal.update(cx, |terminal, _cx| {
            terminal.set_exit_signal(self.exit_tx.clone())
        });
        cx.subscribe(&terminal, Self::on_terminal_event).detach();
        let split = cx.new(|_cx| Split::with_terminal(terminal, profile_id));
        let tab = Tab {
            id: TabId::fresh(),
            split,
            title: title.clone(),
            default_title: title,
            bell_pending: false,
        };

        if replace_initial_tab {
            self.tabs[0] = tab;
            self.active_tab_index = 0;
        } else {
            self.tabs.push(tab);
            self.active_tab_index = self.tabs.len() - 1;
        }
        self.needs_focus = true;
        cx.notify();
    }

    fn on_terminal_event(
        &mut self,
        terminal: Entity<TerminalWidget>,
        event: &TerminalEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_index) = self.tabs.iter().position(|tab| {
            tab.split
                .read(cx)
                .pane_id_for_entity(terminal.entity_id())
                .is_some()
        }) else {
            return;
        };
        let is_active_pane = {
            let split = self.tabs[tab_index].split.read(cx);
            split.pane_id_for_entity(terminal.entity_id()) == Some(split.active_pane_id())
        };
        match event {
            TerminalEvent::TitleChanged(title) if is_active_pane => {
                self.tabs[tab_index].title =
                    crate::shell_integration::display_title(title.as_deref())
                        .unwrap_or_else(|| self.tabs[tab_index].default_title.clone());
            }
            TerminalEvent::Bell if tab_index != self.active_tab_index => {
                self.tabs[tab_index].bell_pending = true;
            }
            TerminalEvent::TitleChanged(_)
            | TerminalEvent::WorkingDirectoryChanged(_)
            | TerminalEvent::Bell => {}
        }
        cx.notify();
    }

    fn active_split(&self) -> Entity<Split> {
        self.tabs[self.active_tab_index].split.clone()
    }

    fn terminal_config(
        &self,
        profile_id: Option<&ProfileId>,
    ) -> Option<(TerminalConfig, ProfileId)> {
        let settings = self.settings.current();
        let profile_id = profile_id.unwrap_or(&settings.default_profile);
        match settings.terminal_config(Some(profile_id)) {
            Ok(config) => Some((config, profile_id.clone())),
            Err(error) => {
                eprintln!("Cannot launch terminal: {error}");
                None
            }
        }
    }

    fn new_terminal(
        &self,
        profile_id: Option<&ProfileId>,
        working_directory: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Option<(Entity<TerminalWidget>, ProfileId)> {
        let (mut config, profile_id) = self.terminal_config(profile_id)?;
        if let Some(working_directory) = working_directory {
            config.launch.working_directory = Some(working_directory);
        }
        Some((
            Self::create_terminal(config, self.exit_tx.clone(), cx),
            profile_id,
        ))
    }

    fn split_active(
        &mut self,
        axis: SplitAxis,
        profile_id: Option<&ProfileId>,
        working_directory: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((new_terminal, profile_id)) = self.new_terminal(profile_id, working_directory, cx)
        else {
            return;
        };
        let split = self.active_split();

        split.update(cx, |split, cx| {
            split.split_active(axis, new_terminal, profile_id, window, cx);
            split.focus_active(window, cx);
        });

        cx.notify();
    }

    fn new_tab(&mut self, profile_id: Option<&ProfileId>, cx: &mut Context<Self>) {
        if self.tabs.len() >= MAX_SELECTABLE_TABS {
            return;
        }

        let settings = self.settings.current();
        let profile_id = profile_id.unwrap_or(&settings.default_profile);
        let Some(profile) = settings.profiles.get(profile_id) else {
            eprintln!("Cannot create tab: profile '{profile_id}' does not exist");
            return;
        };
        let config = settings
            .terminal_config(Some(profile_id))
            .expect("resolved profile has a terminal configuration");
        let tab = Self::create_tab(
            config,
            profile_id.clone(),
            profile.label.clone(),
            self.exit_tx.clone(),
            cx,
        );
        self.tabs.push(tab);
        self.active_tab_index = self.tabs.len() - 1;
        self.needs_focus = true;
        cx.notify();
    }

    fn move_active_tab(&mut self, direction: crate::action::Direction, cx: &mut Context<Self>) {
        let target = match direction {
            crate::action::Direction::Up | crate::action::Direction::Left => {
                self.active_tab_index.checked_sub(1)
            }
            crate::action::Direction::Down | crate::action::Direction::Right => {
                (self.active_tab_index + 1 < self.tabs.len()).then_some(self.active_tab_index + 1)
            }
        };
        let Some(target) = target else {
            return;
        };
        self.tabs.swap(self.active_tab_index, target);
        self.active_tab_index = target;
        cx.notify();
    }

    fn workspace_layout(&self, workspace_id: WorkspaceId, cx: &Context<Self>) -> WorkspaceLayout {
        let tabs = self
            .tabs
            .iter()
            .map(|tab| {
                let split = tab.split.read(cx);
                let panes = split
                    .pane_entities()
                    .map(|(pane_id, profile_id, terminal)| {
                        let working_directory = terminal
                            .read(cx)
                            .workspace_working_directory()
                            .filter(|directory| {
                                trusted_working_directory(directory) && directory.is_dir()
                            });
                        (
                            pane_id,
                            WorkspacePane {
                                profile_id: profile_id.clone(),
                                working_directory,
                            },
                        )
                    })
                    .collect::<BTreeMap<_, _>>();
                WorkspaceTab {
                    id: tab.id,
                    title: tab.title.clone(),
                    active_pane_id: split.active_pane_id(),
                    root: split.topology().clone(),
                    panes,
                }
            })
            .collect();
        WorkspaceLayout::new(workspace_id, self.tabs[self.active_tab_index].id, tabs)
    }

    fn save_workspace(&mut self, workspace_id: WorkspaceId, cx: &mut Context<Self>) {
        let layout = self.workspace_layout(workspace_id.clone(), cx);
        match self.workspace_store.save(&layout) {
            Ok(path) => {
                if !self.workspace_ids.contains(&workspace_id) {
                    self.workspace_ids.push(workspace_id);
                    self.workspace_ids.sort();
                }
                self.workspace_diagnostic = None;
                eprintln!("Workspace saved to {}", path.display());
            }
            Err(error) => {
                self.workspace_diagnostic = Some(format!("Cannot save workspace: {error}"));
            }
        }
        cx.notify();
    }

    fn restore_workspace(&mut self, workspace_id: &WorkspaceId, cx: &mut Context<Self>) {
        let layout = match self.workspace_store.load(workspace_id) {
            Ok(layout) => layout,
            Err(error) => {
                self.workspace_diagnostic = Some(format!("Cannot restore workspace: {error}"));
                cx.notify();
                return;
            }
        };
        let mut warnings = Vec::new();
        let mut tabs = Vec::with_capacity(layout.tabs.len());
        for saved_tab in layout.tabs {
            let WorkspaceTab {
                id,
                title,
                active_pane_id,
                root,
                panes,
            } = saved_tab;
            let mut runtime_panes = Vec::with_capacity(panes.len());
            for (pane_id, pane) in panes {
                let settings = self.settings.current();
                let profile_id = if settings.profiles.contains_key(&pane.profile_id) {
                    pane.profile_id
                } else {
                    warnings.push(format!(
                        "Profile '{}' is missing; used '{}'",
                        pane.profile_id, settings.default_profile
                    ));
                    settings.default_profile.clone()
                };
                let mut config = settings
                    .terminal_config(Some(&profile_id))
                    .expect("resolved workspace profile has a terminal configuration");
                if let Some(directory) = pane.working_directory {
                    if trusted_working_directory(&directory) && directory.is_dir() {
                        config.launch.working_directory = Some(directory);
                    } else {
                        warnings.push(format!(
                            "Working directory for pane {} is unavailable",
                            pane_id.value()
                        ));
                    }
                }
                let terminal = Self::create_terminal(config, self.exit_tx.clone(), cx);
                runtime_panes.push((pane_id, terminal, profile_id));
            }
            let split = Split::from_restored(root, runtime_panes, active_pane_id)
                .expect("validated workspace topology must restore");
            tabs.push(Tab {
                id,
                split: cx.new(|_cx| split),
                title: title.clone(),
                default_title: title,
                bell_pending: false,
            });
        }
        let active_tab_index = tabs
            .iter()
            .position(|tab| tab.id == layout.active_tab_id)
            .expect("validated workspace active tab exists");
        TabId::reserve_after(tabs.iter().map(|tab| tab.id));
        self.tabs = tabs;
        self.active_tab_index = active_tab_index;
        self.workspace_diagnostic = (!warnings.is_empty()).then(|| warnings.join("; "));
        self.needs_focus = true;
        cx.notify();
    }

    fn activate_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }

        self.active_tab_index = index;
        self.tabs[index].bell_pending = false;
        self.needs_focus = true;
        cx.notify();
    }

    fn focus_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let split = self.active_split();
        split.update(cx, |split, cx| split.focus_active(window, cx));
    }

    fn refresh_active_tab_title(&mut self, window: &Window, cx: &mut Context<Self>) {
        let title = self
            .active_terminal(window, cx)
            .and_then(|terminal| {
                crate::shell_integration::display_title(terminal.read(cx).reported_title())
            })
            .unwrap_or_else(|| self.tabs[self.active_tab_index].default_title.clone());
        self.tabs[self.active_tab_index].title = title;
    }

    fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let split = self.active_split();
        let pane_count = split.read(cx).pane_count();

        if pane_count > 1 {
            let pane_to_focus = split.update(cx, |split, cx| split.remove_active_pane(window, cx));
            if let Some(pane) = pane_to_focus {
                pane.update(cx, |pane, _cx| pane.request_focus(window));
            }
            cx.notify();
            return;
        }

        if self.tabs.len() <= 1 {
            cx.quit();
            return;
        }

        self.tabs.remove(self.active_tab_index);
        self.active_tab_index = self
            .active_tab_index
            .saturating_sub(1)
            .min(self.tabs.len() - 1);
        self.needs_focus = true;
        cx.notify();
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    fn start_exit_task(&mut self, exit_rx: flume::Receiver<()>, cx: &mut Context<Self>) {
        self.exit_task = cx.spawn(async move |this, cx| {
            while exit_rx.recv_async().await.is_ok() {
                while exit_rx.try_recv().is_ok() {}

                let Some(this) = this.upgrade() else {
                    break;
                };

                this.update(cx, |this, cx| {
                    if this.remove_exited_panes(cx) {
                        this.needs_focus = true;
                        cx.notify();
                    }
                })
                .ok();
            }
        });
    }

    fn start_settings_task(&mut self, cx: &mut Context<Self>) {
        self.settings_task = cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(2)).await;
                let Some(this) = this.upgrade() else {
                    break;
                };
                let should_continue = this
                    .update(cx, |this, cx| match this.settings.reload_if_changed() {
                        ReloadOutcome::Unchanged => {}
                        ReloadOutcome::Applied { .. } => {
                            let settings = this.settings.current();
                            this.sidebar_visible = settings.app.sidebar_visible;
                            let bindings = settings.key_bindings.clone();
                            for tab in &this.tabs {
                                tab.split.update(cx, |split, cx| {
                                    split.set_action_bindings(&bindings, cx)
                                });
                            }
                            cx.notify();
                        }
                        ReloadOutcome::Rejected(diagnostic) => {
                            eprintln!("Settings reload failed: {diagnostic}");
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !should_continue {
                    break;
                }
            }
        });
    }

    fn remove_exited_panes(&mut self, cx: &mut Context<Self>) -> bool {
        let mut removed_any = false;

        for tab in &self.tabs {
            let exited_panes = tab.split.read(cx).exited_terminal_ids(cx);
            for pane_id in exited_panes {
                if tab.split.read(cx).pane_count() <= 1 {
                    break;
                }

                let removed = tab
                    .split
                    .update(cx, |split, _cx| split.remove_pane_by_entity(pane_id));
                removed_any |= removed.is_some();
            }
        }

        removed_any
    }

    fn on_app_action(
        &mut self,
        action: &DispatchAppAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action.action == AppAction::ToggleQuickTerminal {
            cx.propagate();
            return;
        }
        self.dispatch_app_action(action.action.clone(), window, cx);
    }

    fn dispatch_app_action(
        &mut self,
        action: AppAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            AppAction::NewTab { profile_id } => self.new_tab(profile_id.as_ref(), cx),
            AppAction::Split {
                direction,
                profile_id,
            } => {
                let axis = match direction {
                    ActionSplitDirection::Right => SplitAxis::Horizontal,
                    ActionSplitDirection::Down => SplitAxis::Vertical,
                };
                self.split_active(axis, profile_id.as_ref(), None, window, cx);
            }
            AppAction::SplitFromCurrentDirectory { direction } => {
                let working_directory = self
                    .active_terminal(window, cx)
                    .and_then(|terminal| terminal.read(cx).reported_local_working_directory())
                    .filter(|directory| directory.is_dir());
                if working_directory.is_some() {
                    let axis = match direction {
                        ActionSplitDirection::Right => SplitAxis::Horizontal,
                        ActionSplitDirection::Down => SplitAxis::Vertical,
                    };
                    self.split_active(axis, None, working_directory, window, cx);
                }
            }
            AppAction::ClosePane => self.close_active(window, cx),
            AppAction::Copy => {
                if let Some(terminal) = self.active_terminal(window, cx) {
                    terminal.update(cx, |terminal, cx| terminal.copy_selection(cx));
                }
            }
            AppAction::Paste => {
                if let Some(terminal) = self.active_terminal(window, cx) {
                    terminal.update(cx, |terminal, cx| terminal.paste_clipboard(cx));
                }
            }
            AppAction::JumpToPrompt { direction } => {
                if let Some(terminal) = self.active_terminal(window, cx) {
                    terminal.update(cx, |terminal, cx| terminal.jump_to_prompt(direction, cx));
                }
            }
            AppAction::SelectCommandOutput => {
                if let Some(terminal) = self.active_terminal(window, cx) {
                    terminal.update(cx, |terminal, cx| terminal.select_command_output(cx));
                }
            }
            AppAction::CopyCommandOutput => {
                if let Some(terminal) = self.active_terminal(window, cx) {
                    terminal.update(cx, |terminal, cx| terminal.copy_command_output(cx));
                }
            }
            AppAction::ToggleSidebar => self.toggle_sidebar(cx),
            AppAction::SelectTab { index } => self.activate_tab(usize::from(index), cx),
            AppAction::MoveTab { direction } => self.move_active_tab(direction, cx),
            AppAction::FocusPane { direction } => {
                let split = self.active_split();
                split.update(cx, |split, cx| {
                    split.focus_direction(direction, window, cx);
                });
            }
            AppAction::ResizePane { direction, amount } => {
                let split = self.active_split();
                split.update(cx, |split, cx| {
                    if split.resize_active(direction, amount, window, cx) {
                        cx.notify();
                    }
                });
            }
            AppAction::TogglePaneZoom => {
                let split = self.active_split();
                split.update(cx, |split, cx| {
                    split.toggle_zoom(window, cx);
                    cx.notify();
                });
            }
            AppAction::CommandPalette => self.open_palette(window, cx),
            AppAction::SaveWorkspace { workspace_id } => {
                self.save_workspace(workspace_id, cx);
            }
            AppAction::RestoreWorkspace { workspace_id } => {
                self.restore_workspace(&workspace_id, cx);
            }
            AppAction::Quit => cx.quit(),
            AppAction::ToggleQuickTerminal => {
                window.dispatch_action(
                    Box::new(DispatchAppAction {
                        action: AppAction::ToggleQuickTerminal,
                    }),
                    cx,
                );
            }
            AppAction::Search => {
                if let Some(terminal) = self.active_terminal(window, cx) {
                    terminal.update(cx, |terminal, cx| terminal.open_search(window, cx));
                }
            }
        }
    }

    fn active_terminal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalWidget>> {
        self.active_split()
            .update(cx, |split, cx| split.active_terminal(window, cx))
    }

    fn action_context(&self, window: &Window, cx: &mut Context<Self>) -> ActionContext {
        let split = self.active_split();
        let pane_count = split.read(cx).pane_count();
        let active_terminal = split.update(cx, |split, cx| split.active_terminal(window, cx));
        let has_selection = active_terminal
            .as_ref()
            .is_some_and(|terminal| terminal.read(cx).has_selection());
        let has_local_working_directory = active_terminal.as_ref().is_some_and(|terminal| {
            terminal
                .read(cx)
                .reported_local_working_directory()
                .is_some_and(|directory| directory.is_dir())
        });
        let semantic_commands_available = active_terminal
            .as_ref()
            .is_some_and(|terminal| terminal.read(cx).semantic_commands_available());
        ActionContext {
            has_selection,
            pane_count,
            tab_count: self.tabs.len(),
            has_local_working_directory,
            semantic_commands_available,
            pane_management_available: true,
            search_available: active_terminal.is_some(),
            quick_terminal_available: cfg!(windows)
                && self.settings.current().app.quick_terminal.enabled,
        }
    }

    fn palette_commands(&self, window: &Window, cx: &mut Context<Self>) -> Vec<PaletteCommand> {
        commands(
            self.settings.current(),
            self.action_context(window, cx),
            &self.workspace_ids,
        )
    }

    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.workspace_store.list() {
            Ok(workspaces) => self.workspace_ids = workspaces,
            Err(error) => {
                self.workspace_diagnostic = Some(format!("Cannot list workspaces: {error}"));
            }
        }
        self.palette = Some(PaletteState {
            query: String::new(),
            selected: 0,
        });
        self.palette_focus.focus(window);
        cx.notify();
    }

    fn close_palette(&mut self, cx: &mut Context<Self>) {
        self.palette = None;
        self.needs_focus = true;
        cx.notify();
    }

    fn handle_palette_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(palette) = &self.palette else {
            return;
        };
        let query = palette.query.clone();
        let selected = palette.selected;
        let commands = self.palette_commands(window, cx);
        let matches = filtered_command_indices(&commands, &query);
        match event.keystroke.key.as_str() {
            "escape" => self.close_palette(cx),
            "backspace" => {
                let palette = self.palette.as_mut().expect("palette remains open");
                palette.query.pop();
                palette.selected = 0;
                cx.notify();
            }
            "up" => {
                let palette = self.palette.as_mut().expect("palette remains open");
                palette.selected = palette.selected.saturating_sub(1);
                cx.notify();
            }
            "down" => {
                let palette = self.palette.as_mut().expect("palette remains open");
                palette.selected = (palette.selected + 1).min(matches.len().saturating_sub(1));
                cx.notify();
            }
            "enter" => {
                if let Some(command_index) = matches.get(selected)
                    && commands[*command_index].unavailable_reason.is_none()
                {
                    let action = commands[*command_index].action.clone();
                    self.close_palette(cx);
                    self.dispatch_app_action(action, window, cx);
                }
            }
            _ if !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.alt
                && !event.keystroke.modifiers.platform =>
            {
                if let Some(text) = &event.keystroke.key_char {
                    let palette = self.palette.as_mut().expect("palette remains open");
                    palette.query.push_str(text);
                    palette.selected = 0;
                    cx.notify();
                }
            }
            _ => {}
        }
        window.prevent_default();
        cx.stop_propagation();
    }

    fn handle_palette_key_up(
        &mut self,
        _event: &KeyUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.prevent_default();
        cx.stop_propagation();
    }
}

fn window_control_button(
    id: &'static str,
    area: WindowControlArea,
    button: WindowsCaptionButton,
    is_close: bool,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .h_full()
        .w(px(WINDOW_CONTROL_WIDTH_PX))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .occlude()
        .window_control_area(area)
        .hover(move |style| {
            if is_close {
                style.bg(gpui::rgb(0xc42b1c))
            } else {
                style.bg(gpui::rgb(0x202020))
            }
        })
        .active(move |style| {
            if is_close {
                style.bg(gpui::rgb(0x8f1f14))
            } else {
                style.bg(gpui::rgb(0x2a2a2a))
            }
        })
        .text_size(px(10.0))
        .text_color(gpui::white())
        .line_height(px(TITLE_BAR_HEIGHT_PX))
        .font(caption_icon_font())
        .child(button.icon())
}

impl WindowsCaptionButton {
    fn icon(self) -> &'static str {
        match self {
            Self::Minimize => "\u{e921}",
            Self::Maximize => "\u{e922}",
            Self::Restore => "\u{e923}",
            Self::Close => "\u{e8bb}",
        }
    }
}

fn caption_icon_font() -> Font {
    static CAPTION_ICON_FONT: OnceLock<Font> = OnceLock::new();

    CAPTION_ICON_FONT
        .get_or_init(|| {
            let mut icon_font = font(caption_icon_font_family());
            icon_font.fallbacks = Some(FontFallbacks::from_fonts(vec![
                "Segoe MDL2 Assets".to_string(),
            ]));
            icon_font
        })
        .clone()
}

#[cfg(target_os = "windows")]
fn caption_icon_font_family() -> &'static str {
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

    let mut version: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
    version.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;

    let status = unsafe { RtlGetVersion(&mut version) };
    if status >= 0 && version.dwBuildNumber >= 22000 {
        "Segoe Fluent Icons"
    } else {
        "Segoe MDL2 Assets"
    }
}

#[cfg(not(target_os = "windows"))]
fn caption_icon_font_family() -> &'static str {
    "Segoe Fluent Icons"
}

impl Render for PaneContainer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_focus {
            self.needs_focus = false;
            self.focus_active_tab(window, cx);
        }
        self.refresh_active_tab_title(window, cx);
        let palette = self
            .palette
            .as_ref()
            .map(|_| self.render_command_palette(window, cx));

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(gpui::rgb(WINDOW_BACKGROUND))
            .on_action(cx.listener(Self::on_app_action))
            .child(render_titlebar(window))
            .children(
                self.settings
                    .diagnostic()
                    .map(|diagnostic| render_settings_diagnostic(diagnostic.to_string())),
            )
            .children(
                self.workspace_diagnostic
                    .clone()
                    .map(render_workspace_diagnostic),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .pl(px(WINDOW_HORIZONTAL_PADDING_PX))
                    .pr(px(WINDOW_HORIZONTAL_PADDING_PX))
                    .flex()
                    .flex_row()
                    .children(self.sidebar_visible.then(|| self.render_sidebar(cx)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.active_split()),
                    ),
            )
            .children(palette)
    }
}

fn render_settings_diagnostic(message: String) -> impl IntoElement {
    div()
        .px(px(10.0))
        .py(px(6.0))
        .bg(gpui::rgb(0x5a1717))
        .text_color(gpui::rgb(0xffffff))
        .text_size(px(12.0))
        .child(format!("Settings error: {message}"))
}

fn render_workspace_diagnostic(message: String) -> impl IntoElement {
    div()
        .px(px(10.0))
        .py(px(6.0))
        .bg(gpui::rgb(0x4c3414))
        .text_color(gpui::rgb(0xffffff))
        .text_size(px(12.0))
        .child(format!("Workspace: {message}"))
}

fn render_titlebar(window: &mut Window) -> impl IntoElement {
    let maximize_button = if window.is_maximized() {
        WindowsCaptionButton::Restore
    } else {
        WindowsCaptionButton::Maximize
    };

    div()
        .h(px(TITLE_BAR_HEIGHT_PX))
        .flex()
        .flex_row()
        .items_center()
        .bg(gpui::rgb(WINDOW_BACKGROUND))
        .when(cfg!(target_os = "macos"), |titlebar| {
            titlebar
                .child(
                    div()
                        .id("mac-traffic-light-space")
                        .h_full()
                        .w(px(MAC_TRAFFIC_LIGHT_SPACER_PX))
                        .flex_shrink_0()
                        .window_control_area(WindowControlArea::Drag)
                        .on_double_click(|_, window, _| window.titlebar_double_click()),
                )
                .child(
                    div()
                        .id("titlebar-drag")
                        .h_full()
                        .flex_1()
                        .window_control_area(WindowControlArea::Drag)
                        .on_double_click(|_, window, _| window.titlebar_double_click()),
                )
        })
        .when(!cfg!(target_os = "macos"), |titlebar| {
            titlebar
                .child(
                    div()
                        .id("titlebar-drag")
                        .h_full()
                        .flex_1()
                        .window_control_area(WindowControlArea::Drag)
                        .on_double_click(|_, window, _| window.zoom_window()),
                )
                .child(window_control_button(
                    "minimize",
                    WindowControlArea::Min,
                    WindowsCaptionButton::Minimize,
                    false,
                ))
                .child(window_control_button(
                    "maximize",
                    WindowControlArea::Max,
                    maximize_button,
                    false,
                ))
                .child(window_control_button(
                    "close",
                    WindowControlArea::Close,
                    WindowsCaptionButton::Close,
                    true,
                ))
        })
}

impl PaneContainer {
    fn render_command_palette(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = self
            .palette
            .as_ref()
            .expect("palette render requires open state");
        let commands = self.palette_commands(window, cx);
        let matches = filtered_command_indices(&commands, &palette.query);
        let selected = palette.selected.min(matches.len().saturating_sub(1));
        let query = palette.query.clone();

        div()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(72.0))
            .bg(gpui::rgba(0x00000099))
            .occlude()
            .track_focus(&self.palette_focus)
            .on_key_down(cx.listener(Self::handle_palette_key_down))
            .on_key_up(cx.listener(Self::handle_palette_key_up))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                    this.close_palette(cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .w(px(620.0))
                    .max_h(px(560.0))
                    .overflow_hidden()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(gpui::rgb(0x383838))
                    .bg(gpui::rgb(0x151515))
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_event: &MouseDownEvent, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .h(px(48.0))
                            .px(px(14.0))
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(gpui::rgb(0x303030))
                            .text_size(px(15.0))
                            .text_color(gpui::rgb(0xf2f2f2))
                            .child(if query.is_empty() {
                                "Type a command…".to_string()
                            } else {
                                format!("{query}▏")
                            }),
                    )
                    .children(
                        matches
                            .into_iter()
                            .take(PALETTE_MAX_RESULTS)
                            .enumerate()
                            .map(|(result_index, command_index)| {
                                let command = &commands[command_index];
                                let action = command.action.clone();
                                let available = command.unavailable_reason.is_none();
                                let detail = command
                                    .unavailable_reason
                                    .map(ToOwned::to_owned)
                                    .or_else(|| command.binding.clone())
                                    .unwrap_or_else(|| command.category.title().to_string());

                                div()
                                    .id(("palette-command", command_index))
                                    .h(px(40.0))
                                    .px(px(12.0))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .bg(if result_index == selected {
                                        gpui::rgb(0x292929)
                                    } else {
                                        gpui::rgb(0x151515)
                                    })
                                    .text_color(if available {
                                        gpui::rgb(0xeeeeee)
                                    } else {
                                        gpui::rgb(0x777777)
                                    })
                                    .when(available, |row| {
                                        row.hover(|style| style.bg(gpui::rgb(0x242424)))
                                            .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(
                                                move |this, _event: &MouseDownEvent, window, cx| {
                                                    this.close_palette(cx);
                                                    this.dispatch_app_action(
                                                        action.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                    window.prevent_default();
                                                    cx.stop_propagation();
                                                },
                                            ),
                                        )
                                    })
                                    .child(div().text_size(px(13.0)).child(command.title.clone()))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(if available {
                                                gpui::rgb(0x999999)
                                            } else {
                                                gpui::rgb(0x6f6f6f)
                                            })
                                            .child(detail),
                                    )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(SIDEBAR_WIDTH_PX))
            .h_full()
            .flex_shrink_0()
            .mr(px(SIDEBAR_GAP_PX))
            .bg(gpui::rgb(WINDOW_BACKGROUND))
            .pt(px(4.0))
            .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                let is_active = index == self.active_tab_index;
                let label = if tab.bell_pending {
                    format!("{}•", index + 1)
                } else {
                    (index + 1).to_string()
                };
                let title = tab.title.clone();

                div()
                    .id(("tab", index))
                    .h(px(TAB_HEIGHT_PX))
                    .w_full()
                    .mb(px(4.0))
                    .rounded(px(TAB_RADIUS_PX))
                    .overflow_hidden()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .text_color(if is_active {
                        gpui::rgb(0xf0f0f0)
                    } else {
                        gpui::rgb(0x8a8a8a)
                    })
                    .bg(if is_active {
                        gpui::rgb(0x1a1a1a)
                    } else {
                        gpui::rgb(WINDOW_BACKGROUND)
                    })
                    .hover(|style| style.bg(gpui::rgb(0x202020)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                            this.activate_tab(index, cx);
                        }),
                    )
                    .child(
                        div()
                            .w(px(18.0))
                            .flex_shrink_0()
                            .text_center()
                            .text_size(px(13.0))
                            .line_height(px(TAB_HEIGHT_PX))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .w_full()
                            .truncate()
                            .text_size(px(12.0))
                            .line_height(px(TAB_HEIGHT_PX))
                            .child(title),
                    )
            }))
    }
}
