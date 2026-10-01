use gpui::{
    AnyElement, Context, Entity, FocusHandle, Font, FontFallbacks, IntoElement, KeyDownEvent,
    KeyUpEvent, MouseButton, MouseDownEvent, Render, Task, Timer, Window, WindowControlArea, div,
    font, prelude::*, px,
};
use gpui_component::{
    InteractiveElementExt,
    button::{Button, ButtonVariants},
    menu::AppMenuBar,
};
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
mod control;
mod wait;
pub use wait::ControlWait;

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
    window_id: String,
    content_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    layout_input: Option<(gpui::Size<gpui::Pixels>, bool)>,
    control_acks: Vec<flume::Receiver<crate::control::Acknowledgement>>,
    tabs: Vec<Tab>,
    active_tab_index: usize,
    sidebar_visible: bool,
    label_geometry: std::rc::Rc<std::cell::RefCell<BTreeMap<u64, serde_json::Value>>>,
    needs_focus: bool,
    titlebar_visible: bool,
    app_menu_bar: Option<Entity<AppMenuBar>>,
    bell_notification_pending: bool,
    settings: SettingsStore,
    settings_task: Task<()>,
    workspace_store: WorkspaceStore,
    workspace_ids: Vec<WorkspaceId>,
    workspace_diagnostic: Option<String>,
    palette: Option<PaletteState>,
    palette_restore_focus: Option<gpui::WeakFocusHandle>,
    palette_focus: FocusHandle,
    palette_input: Option<Entity<gpui_component::input::InputState>>,
    palette_input_subscription: Option<gpui::Subscription>,
    exit_tx: flume::Sender<()>,
    exit_task: Task<()>,
}

impl PaneContainer {
    pub fn take_control_acks(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<flume::Receiver<crate::control::Acknowledgement>> {
        let mut acks = std::mem::take(&mut self.control_acks);
        for tab in &self.tabs {
            acks.extend(tab.split.update(cx, |split, _| split.take_control_acks()));
        }
        acks
    }
    pub fn establish_layout(&mut self, window: &Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        if self.layout_input == Some((viewport, self.sidebar_visible))
            && let Some(bounds) = self.content_bounds
        {
            self.apply_content_layout(bounds, window, cx);
            return;
        }
        self.layout_input = Some((viewport, self.sidebar_visible));
        let sidebar = if self.sidebar_visible {
            SIDEBAR_WIDTH_PX + SIDEBAR_GAP_PX
        } else {
            0.0
        };
        let titlebar = if self.titlebar_visible {
            TITLE_BAR_HEIGHT_PX
        } else {
            0.0
        };
        let bounds = gpui::Bounds {
            origin: gpui::point(px(WINDOW_HORIZONTAL_PADDING_PX + sidebar), px(titlebar)),
            size: gpui::size(
                (viewport.width - px(2.0 * WINDOW_HORIZONTAL_PADDING_PX + sidebar)).max(px(1.0)),
                (viewport.height - px(titlebar)).max(px(1.0)),
            ),
        };
        self.apply_content_layout(bounds, window, cx);
    }
    fn apply_content_layout(
        &mut self,
        bounds: gpui::Bounds<gpui::Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.content_bounds = Some(bounds);
        for tab in &self.tabs {
            tab.split
                .update(cx, |split, cx| split.apply_layout(bounds, window, cx));
        }
    }
    fn identify_launch(
        config: &mut TerminalConfig,
        window_id: &str,
        tab: TabId,
        pane: crate::split::PaneId,
    ) {
        for (key, value) in [
            (
                "MIGHTTY_INSTANCE_ID",
                crate::control::instance_id().to_string(),
            ),
            ("MIGHTTY_WINDOW_ID", window_id.to_string()),
            ("MIGHTTY_TAB_ID", format!("t{}", tab.value())),
            ("MIGHTTY_PANE_ID", format!("p{}", pane.value())),
        ] {
            config.launch.environment.insert(key.into(), value.into());
        }
    }
    pub fn control_contains(&self, target: &crate::control::Target, cx: &gpui::App) -> bool {
        self.tabs.iter().any(|tab| {
            target
                .tab_id
                .as_ref()
                .is_none_or(|id| id == "active" || *id == format!("t{}", tab.id.value()))
                && target.pane_id.as_ref().is_none_or(|id| {
                    id == "active"
                        || tab
                            .split
                            .read(cx)
                            .pane_entities()
                            .any(|(pane, _, _)| *id == format!("p{}", pane.value()))
                })
        })
    }

    fn control_target(
        &self,
        target: &crate::control::Target,
        cx: &gpui::App,
    ) -> Result<(usize, crate::split::PaneId, Entity<TerminalWidget>), crate::control::ControlError>
    {
        use crate::control::ControlError;
        let mut matches = Vec::new();
        for (index, tab) in self.tabs.iter().enumerate() {
            if target.tab_id.as_ref().is_some_and(|id| {
                if id == "active" {
                    index != self.active_tab_index
                } else {
                    *id != format!("t{}", tab.id.value())
                }
            }) {
                continue;
            }
            let split = tab.split.read(cx);
            for (pane, _, terminal) in split.pane_entities() {
                if target.pane_id.as_ref().is_some_and(|id| {
                    if id == "active" {
                        (target.tab_id.is_none() && index != self.active_tab_index)
                            || pane != split.active_pane_id()
                    } else {
                        *id != format!("p{}", pane.value())
                    }
                }) {
                    continue;
                }
                if target.pane_id.is_none()
                    && target.tab_id.is_some()
                    && pane != split.active_pane_id()
                {
                    continue;
                }
                matches.push((index, pane, terminal));
            }
        }
        match matches.len() {
            1 => Ok(matches.remove(0)),
            0 => Err(ControlError::new(
                "target_unavailable",
                "stale or conflicting tab/pane target",
            )),
            _ => Err(ControlError::new(
                "ambiguous_target",
                "specify a tab or pane ID, or active",
            )),
        }
    }

    pub fn window_id(&self) -> &str {
        &self.window_id
    }

    pub fn control_state(&self, window: &Window, cx: &gpui::App) -> serde_json::Value {
        use serde_json::json;
        json!({"bounds":{"width":f32::from(window.viewport_size().width),"height":f32::from(window.viewport_size().height)},
            "visibility":crate::snapshot::visibility(window),
            "layout_token":self.window_layout_token(window,cx),
            "dpi_scale":window.scale_factor(),"os_focused":window.is_window_active(),
            "active_tab_id":format!("t{}",self.tabs[self.active_tab_index].id.value()),
            "sidebar_visible":self.sidebar_visible,"palette_open":self.palette.is_some(),"palette":self.palette.as_ref().map(|palette|json!({"query":palette.query,"selected":palette.selected})),"settings_generation":self.settings.generation().to_string(),
            "tabs":self.tabs.iter().enumerate().map(|(index,tab)| {let split=tab.split.read(cx);json!({"tab_id":format!("t{}",tab.id.value()),"title":tab.title,"fallback_title":tab.default_title,"title_provenance":self.title_provenance(index,cx),
                "selected_pane_id":format!("p{}",split.active_pane_id().value()),"zoomed_pane_id":split.zoomed_pane_id().map(|p|format!("p{}",p.value())),"topology":control::topology(split.topology()),"layout_token":split.layout_token(tab.id),
                "panes":split.pane_entities().map(|(id,profile,terminal)|{let mut state=terminal.read(cx).control_state();let pane_id=format!("p{}",id.value());state["last_presented"]=crate::snapshot::pane_presentation(window,&pane_id);state["pane_id"]=json!(pane_id);state["profile_id"]=json!(profile.as_str());state["output_cursor"]=json!(format!("{}:p{}:{}",crate::control::instance_id(),id.value(),state["output_seq"].as_str().unwrap()));state}).collect::<Vec<_>>()})}).collect::<Vec<_>>()})
    }

    pub fn control_dispatch(
        &mut self,
        request: &crate::control::Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<serde_json::Value, crate::control::ControlError> {
        use serde_json::json;
        match request.op.as_str() {
            "capabilities" => Ok(crate::control::capabilities()),
            "profiles" => Ok(json!(self.settings.current().profiles.iter().map(|(id,profile)|json!({"profile_id":id.as_str(),"label":profile.label,"executable":profile.launch.executable,"argv":profile.launch.arguments.iter().map(|v|v.to_string_lossy()).collect::<Vec<_>>(),"working_directory":profile.launch.working_directory})).collect::<Vec<_>>())),
            "state" => {
                if request.target.tab_id.is_none() && request.target.pane_id.is_none() {return Ok(self.control_state(window,cx));}
                let (index,pane,terminal)=self.control_target(&request.target,cx)?;
                if request.target.pane_id.is_none() {return Ok(self.control_state(window,cx)["tabs"][index].clone());}
                let mut result=terminal.read(cx).control_state();let pane_id=format!("p{}",pane.value());result["last_presented"]=crate::snapshot::pane_presentation(window,&pane_id);result["tab_id"]=json!(format!("t{}",self.tabs[index].id.value()));result["pane_id"]=json!(pane_id);result["output_cursor"]=json!(format!("{}:p{}:{}",crate::control::instance_id(),pane.value(),result["output_seq"].as_str().unwrap()));Ok(result)
            }
            "pane.read" => {let (index,pane,terminal)=self.targeted_terminal(request,cx)?;let mut result=terminal.update(cx,|terminal,_|terminal.control_read(request))?;
                result["tab_id"]=json!(format!("t{}",self.tabs[index].id.value()));result["pane_id"]=json!(format!("p{}",pane.value()));result["output_cursor"]=json!(format!("{}:p{}:{}",crate::control::instance_id(),pane.value(),result["output_seq"].as_str().unwrap()));Ok(result)}
            _ => self.control_mutation(request,window,cx),
        }
    }

    pub fn new(settings: SettingsStore, cx: &mut Context<Self>) -> Self {
        Self::with_titlebar(settings, true, cx)
    }

    /// Create terminal content for a frameless quick-terminal window.
    pub fn new_without_titlebar(settings: SettingsStore, cx: &mut Context<Self>) -> Self {
        Self::with_titlebar(settings, false, cx)
    }

    fn with_titlebar(
        settings: SettingsStore,
        titlebar_visible: bool,
        cx: &mut Context<Self>,
    ) -> Self {
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
        static NEXT_WINDOW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let window_id = format!(
            "w{}",
            NEXT_WINDOW.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let tab = Self::create_tab(config, profile_id, title, &window_id, exit_tx.clone(), cx);

        let workspace_store = WorkspaceStore::open_default();
        let workspace_ids = workspace_store.list().unwrap_or_default();
        let mut container = Self {
            window_id,
            content_bounds: None,
            layout_input: None,
            control_acks: Vec::new(),
            tabs: vec![tab],
            active_tab_index: 0,
            sidebar_visible,
            label_geometry: Default::default(),
            needs_focus: true,
            titlebar_visible,
            app_menu_bar: None,
            bell_notification_pending: false,
            settings,
            settings_task: Task::ready(()),
            workspace_store,
            workspace_ids,
            workspace_diagnostic: None,
            palette: None,
            palette_restore_focus: None,
            palette_focus: cx.focus_handle(),
            palette_input: None,
            palette_input_subscription: None,
            exit_tx,
            exit_task: Task::ready(()),
        };
        container.start_exit_task(exit_rx, cx);
        container.start_settings_task(cx);
        container
    }

    fn create_tab(
        mut config: TerminalConfig,
        profile_id: ProfileId,
        title: String,
        window_id: &str,
        exit_tx: flume::Sender<()>,
        cx: &mut Context<Self>,
    ) -> Tab {
        let id = TabId::fresh();
        let pane_id = crate::split::PaneId::allocate();
        Self::identify_launch(&mut config, window_id, id, pane_id);
        let terminal = Self::create_terminal(config, exit_tx, cx);
        let split = cx.new(|_cx| Split::with_terminal_id(pane_id, terminal, profile_id));

        Tab {
            id,
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
        cx.observe(&terminal, |_, _, _| crate::diagnostics::mark_dirty())
            .detach();
        terminal
    }

    /// Open a Windows terminal handoff as an active tab.
    ///
    /// The first handoff can replace the one tab created for a hidden COM start.
    /// Later handoffs add a tab without removing user sessions.
    #[cfg(windows)]
    pub fn can_open_handoff(&self, replace_existing: bool) -> bool {
        (replace_existing && self.tabs.len() == 1) || self.tabs.len() < MAX_SELECTABLE_TABS
    }

    #[cfg(windows)]
    pub fn open_handoff(
        &mut self,
        parts: PtyParts,
        startup_title: Option<String>,
        replace_existing: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let replace_initial_tab = replace_existing && self.tabs.len() == 1;
        if !self.can_open_handoff(replace_existing) {
            return false;
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
        cx.observe(&terminal, |_, _, _| crate::diagnostics::mark_dirty())
            .detach();
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
        true
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
            TerminalEvent::Bell => {
                if tab_index != self.active_tab_index {
                    self.tabs[tab_index].bell_pending = true;
                }
                self.bell_notification_pending = true;
            }
            TerminalEvent::TitleChanged(_) | TerminalEvent::WorkingDirectoryChanged(_) => {}
        }
        cx.notify();
    }

    fn deliver_bell_notification(&mut self, window: &Window) {
        if !self.bell_notification_pending {
            return;
        }
        self.bell_notification_pending = false;
        if !bell_notification_allowed(
            self.settings.current().app.bell_notifications,
            window.is_window_active(),
        ) {
            return;
        }

        #[cfg(windows)]
        if let Err(error) = crate::application::windows::request_window_attention(window) {
            eprintln!("Cannot request attention for a terminal bell: {error}");
        }
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
    ) -> Option<(crate::split::PaneId, Entity<TerminalWidget>, ProfileId)> {
        let (mut config, profile_id) = self.terminal_config(profile_id)?;
        if let Some(working_directory) = working_directory {
            config.launch.working_directory = Some(working_directory);
        }
        let pane_id = crate::split::PaneId::allocate();
        Self::identify_launch(
            &mut config,
            &self.window_id,
            self.tabs[self.active_tab_index].id,
            pane_id,
        );
        Some((
            pane_id,
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
        let Some((pane_id, new_terminal, profile_id)) =
            self.new_terminal(profile_id, working_directory, cx)
        else {
            return;
        };
        let split = self.active_split();

        split.update(cx, |split, cx| {
            let direction = match axis {
                SplitAxis::Horizontal => crate::action::Direction::Right,
                SplitAxis::Vertical => crate::action::Direction::Down,
            };
            split.split_target(
                split.active_pane_id(),
                pane_id,
                direction,
                0.5,
                (new_terminal, profile_id),
                true,
            );
            split.focus_active(window, cx);
        });
        self.establish_layout(window, cx);

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
            &self.window_id,
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
        let saved_active_tab = layout.active_tab_id;
        let mut active_tab_index = 0;
        for saved_tab in layout.tabs {
            let WorkspaceTab {
                id: saved_id,
                title,
                active_pane_id,
                mut root,
                panes,
            } = saved_tab;
            let id = TabId::fresh();
            if saved_id == saved_active_tab {
                active_tab_index = tabs.len();
            }
            let mapping = panes
                .keys()
                .map(|saved| (*saved, crate::split::PaneId::allocate()))
                .collect::<BTreeMap<_, _>>();
            root.remap_ids(&mapping);
            let active_pane_id = mapping[&active_pane_id];
            let mut runtime_panes = Vec::with_capacity(panes.len());
            for (pane_id, pane) in panes {
                let pane_id = mapping[&pane_id];
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
                Self::identify_launch(&mut config, &self.window_id, id, pane_id);
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
            self.retain_pane(
                self.active_tab_index,
                split.read(cx).active_pane_id(),
                "pane_closed",
                cx,
            );
            let pane_to_focus = split.update(cx, |split, cx| split.remove_active_pane(window, cx));
            if let Some(pane) = pane_to_focus {
                pane.update(cx, |pane, _cx| pane.request_focus(window));
            }
            cx.notify();
            return;
        }

        self.close_tab_target(self.active_tab_index, window, cx);
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
                            let generation = this.settings.generation();
                            for tab in &this.tabs {
                                tab.split.update(cx, |split, cx| {
                                    split.set_action_bindings(&bindings, generation, cx)
                                });
                            }
                            cx.notify();
                        }
                        ReloadOutcome::Rejected(diagnostic) => {
                            crate::diagnostics::record(
                                "settings",
                                "reload_rejected",
                                &diagnostic.to_string(),
                                serde_json::json!({"window_id":this.window_id}),
                            );
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

        for index in 0..self.tabs.len() {
            let split = self.tabs[index].split.clone();
            let exited_panes = split.read(cx).exited_terminal_ids(cx);
            for pane_id in exited_panes {
                if split.read(cx).pane_count() <= 1 {
                    break;
                }

                let id = split
                    .read(cx)
                    .pane_entities()
                    .find(|(_, _, terminal)| terminal.entity_id() == pane_id)
                    .map(|(id, _, _)| id);
                if let Some(id) = id {
                    self.retain_pane(index, id, "output_ended", cx);
                }
                let removed = split.update(cx, |split, _cx| split.remove_pane_by_entity(pane_id));
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
        if self.palette.is_none() {
            self.palette_restore_focus = window.focused(cx).map(|focus| focus.downgrade());
        }
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
        let input = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx).placeholder("Type a command…")
        });
        self.palette_input_subscription =
            Some(
                cx.subscribe_in(&input, window, |this, input, event, _window, cx| {
                    if let gpui_component::input::InputEvent::Change = event
                        && let Some(palette) = this.palette.as_mut()
                    {
                        palette.query = input.read(cx).value().to_string();
                        palette.selected = 0;
                        cx.notify();
                    }
                }),
            );
        input.update(cx, |input, cx| input.focus(window, cx));
        self.palette_input = Some(input);
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.palette_input = None;
        self.palette_input_subscription = None;
        if let Some(focus) = self
            .palette_restore_focus
            .take()
            .and_then(|focus| focus.upgrade())
        {
            focus.focus(window);
            self.needs_focus = false;
        } else {
            self.needs_focus = true;
        }
        cx.notify();
    }

    fn handle_palette_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_palette_navigation(&event.keystroke.key, window, cx);
    }

    fn handle_palette_navigation(
        &mut self,
        key: &str,
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
        match key {
            "escape" => self.close_palette(window, cx),
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
                    self.close_palette(window, cx);
                    self.dispatch_app_action(action, window, cx);
                }
            }
            _ => return,
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
        self.establish_layout(window, cx);
        if self.needs_focus {
            self.needs_focus = false;
            self.focus_active_tab(window, cx);
        }
        self.refresh_active_tab_title(window, cx);
        self.deliver_bell_notification(window);
        if self.titlebar_visible && !cfg!(target_os = "macos") && self.app_menu_bar.is_none() {
            self.app_menu_bar = Some(AppMenuBar::new(window, cx));
        }
        let app_menu_bar = self.app_menu_bar.clone();
        let sidebar_toggle = Button::new("sidebar-toggle")
            .ghost()
            .w(px(34.0))
            .h(px(28.0))
            .p_0()
            .mx(px(3.0))
            .tooltip(if self.sidebar_visible {
                "Hide sidebar"
            } else {
                "Show sidebar"
            })
            .child(
                div()
                    .w(px(18.0))
                    .h(px(14.0))
                    .border_1()
                    .border_color(gpui::rgb(0xb0b0b0))
                    .rounded(px(2.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .w(px(5.0))
                            .h_full()
                            .border_r_1()
                            .border_color(gpui::rgb(0xb0b0b0))
                            .when(self.sidebar_visible, |icon| icon.bg(gpui::rgb(0xb0b0b0))),
                    ),
            )
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(cx.listener(|this, _, window, cx| {
                this.dispatch_app_action(AppAction::ToggleSidebar, window, cx);
            }))
            .into_any_element();
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
            .children(
                self.titlebar_visible
                    .then(|| render_titlebar(window, app_menu_bar, sidebar_toggle)),
            )
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
                            .relative()
                            .child(
                                gpui::canvas(
                                    {
                                        let entity = cx.entity();
                                        move |bounds, window, cx| {
                                            entity.update(cx, |this, cx| {
                                                this.apply_content_layout(bounds, window, cx)
                                            })
                                        }
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
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

fn render_titlebar(
    window: &mut Window,
    app_menu_bar: Option<Entity<AppMenuBar>>,
    sidebar_toggle: AnyElement,
) -> impl IntoElement {
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
            titlebar.child(
                div()
                    .id("mac-traffic-light-space")
                    .h_full()
                    .w(px(MAC_TRAFFIC_LIGHT_SPACER_PX))
                    .flex_shrink_0()
                    .window_control_area(WindowControlArea::Drag)
                    .on_double_click(|_, window, _| window.titlebar_double_click()),
            )
        })
        .child(sidebar_toggle)
        .when(cfg!(target_os = "macos"), |titlebar| {
            titlebar.child(
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
                .children(
                    app_menu_bar.map(|menu_bar| {
                        div().h_full().w(px(300.0)).flex_shrink_0().child(menu_bar)
                    }),
                )
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

fn bell_notification_allowed(enabled: bool, window_active: bool) -> bool {
    enabled && !window_active
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
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::MoveUp, window, cx| {
                    this.handle_palette_navigation("up", window, cx);
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::MoveDown, window, cx| {
                    this.handle_palette_navigation("down", window, cx);
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::Enter, window, cx| {
                    this.handle_palette_navigation("enter", window, cx);
                }),
            )
            .on_key_down(cx.listener(Self::handle_palette_key_down))
            .on_key_up(cx.listener(Self::handle_palette_key_up))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                    this.close_palette(window, cx);
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
                            .children(
                                self.palette_input
                                    .as_ref()
                                    .map(gpui_component::input::Input::new),
                            ),
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
                                                    this.close_palette(window, cx);
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
                let geometry = self.label_geometry.clone();
                let id = tab.id.value();

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
                            .relative()
                            .text_size(px(12.0))
                            .line_height(px(TAB_HEIGHT_PX))
                            .child(title)
                            .child(gpui::canvas(move |bounds, window, _| {
                                let style = window.text_style();
                                geometry.borrow_mut().insert(id, serde_json::json!({"bounds":crate::snapshot::rect(bounds),"ancestor_clip":crate::snapshot::rect(window.content_mask().bounds),"font_family":style.font_family.as_ref(),"font_size_px":12,"line_height_px":TAB_HEIGHT_PX,"wrap":"nowrap","overflow":"ellipsis"}));
                            }, |_, _, _, _| {}).absolute().top(px(0.)).left(px(0.)).size_full()),
                    )
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bell_notifications_follow_policy_and_window_focus() {
        assert!(bell_notification_allowed(true, false));
        assert!(!bell_notification_allowed(true, true));
        assert!(!bell_notification_allowed(false, false));
    }

    #[gpui::test]
    fn palette_native_input_preserves_query_editing_and_navigation(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        use std::{cell::RefCell, rc::Rc};
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::widget::init(cx);
        });
        let slot = Rc::new(RefCell::new(None));
        let build_slot = slot.clone();
        let (_root, cx) = cx.add_window_view(move |window, cx| {
            let container = cx.new(|cx| {
                PaneContainer::new_without_titlebar(
                    SettingsStore::open(
                        std::env::temp_dir()
                            .join(format!("mightty-palette-test-{}.json", std::process::id())),
                    ),
                    cx,
                )
            });
            build_slot.replace(Some(container.clone()));
            gpui_component::Root::new(container, window, cx)
        });
        let container = slot.borrow().clone().unwrap();
        cx.refresh().unwrap();
        cx.update_window_entity(&container, |container, window, cx| {
            container.open_palette(window, cx)
        });
        cx.refresh().unwrap();
        cx.simulate_input("日本😀");
        cx.run_until_parked();
        cx.update_window_entity(&container, |container, _, _| {
            assert_eq!(container.palette.as_ref().unwrap().query, "日本😀")
        });
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("sidebar");
        cx.run_until_parked();
        cx.simulate_keystrokes("down up");
        let visible = cx.update_window_entity(&container, |container, _, _| {
            assert_eq!(container.palette.as_ref().unwrap().query, "sidebar");
            assert_eq!(container.palette.as_ref().unwrap().selected, 0);
            container.sidebar_visible
        });
        cx.simulate_keystrokes("enter");
        cx.update_window_entity(&container, |container, _, _| {
            assert!(container.palette.is_none());
            assert_eq!(container.sidebar_visible, !visible);
        });
    }
}
