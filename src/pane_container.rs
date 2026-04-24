use gpui::{
    Action, App, Context, Entity, Font, FontFallbacks, IntoElement, KeyBinding, MouseButton,
    MouseDownEvent, Render, Window, WindowControlArea, actions, div, font, prelude::*, px,
};
use gpui_component::InteractiveElementExt;
use serde::Deserialize;
use std::path::Path;
use std::sync::OnceLock;

use crate::pane::Pane;
use crate::split::{Split, SplitDirection};
use crate::widget::TerminalConfig;

actions!(
    terminal,
    [SplitRight, SplitDown, NewTab, CloseActive, ToggleSidebar]
);

#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = terminal, no_json)]
pub struct SelectTab {
    pub index: usize,
}

const WINDOW_BACKGROUND: u32 = 0x000000;
const WINDOW_HORIZONTAL_PADDING_PX: f32 = 8.0;
const TITLE_BAR_HEIGHT_PX: f32 = 34.0;
const WINDOW_CONTROL_WIDTH_PX: f32 = 46.0;
const SIDEBAR_WIDTH_PX: f32 = 160.0;
const SIDEBAR_GAP_PX: f32 = 8.0;
const TAB_RADIUS_PX: f32 = 4.0;
const TAB_HEIGHT_PX: f32 = 42.0;
const MAX_SELECTABLE_TABS: usize = 9;

#[derive(Clone, Copy)]
enum WindowsCaptionButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

struct Tab {
    split: Entity<Split>,
    title: String,
}

pub struct PaneContainer {
    tabs: Vec<Tab>,
    active_tab_index: usize,
    sidebar_visible: bool,
    needs_focus: bool,
    config: TerminalConfig,
}

impl PaneContainer {
    pub fn new(config: TerminalConfig, cx: &mut Context<Self>) -> Self {
        let tab = Self::create_tab(config.clone(), cx);

        Self {
            tabs: vec![tab],
            active_tab_index: 0,
            sidebar_visible: true,
            needs_focus: true,
            config,
        }
    }

    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("alt-enter", SplitRight, None),
            KeyBinding::new("alt-shift-enter", SplitDown, None),
            KeyBinding::new("ctrl-t", NewTab, None),
            KeyBinding::new("ctrl-d", CloseActive, None),
            KeyBinding::new("ctrl-b", ToggleSidebar, None),
            KeyBinding::new("ctrl-1", SelectTab { index: 0 }, None),
            KeyBinding::new("ctrl-2", SelectTab { index: 1 }, None),
            KeyBinding::new("ctrl-3", SelectTab { index: 2 }, None),
            KeyBinding::new("ctrl-4", SelectTab { index: 3 }, None),
            KeyBinding::new("ctrl-5", SelectTab { index: 4 }, None),
            KeyBinding::new("ctrl-6", SelectTab { index: 5 }, None),
            KeyBinding::new("ctrl-7", SelectTab { index: 6 }, None),
            KeyBinding::new("ctrl-8", SelectTab { index: 7 }, None),
            KeyBinding::new("ctrl-9", SelectTab { index: 8 }, None),
        ]);
    }

    fn create_tab(config: TerminalConfig, cx: &mut Context<Self>) -> Tab {
        let pane = cx.new(|cx| Pane::new(config, cx));
        let split = cx.new(|_cx| Split::with_pane(pane));

        Tab {
            split,
            title: short_current_dir_title(),
        }
    }

    fn active_split(&self) -> Entity<Split> {
        self.tabs[self.active_tab_index].split.clone()
    }

    fn new_pane(&self, cx: &mut Context<Self>) -> Entity<Pane> {
        cx.new(|cx| Pane::new(self.config.clone(), cx))
    }

    fn split_active(
        &mut self,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_pane = self.new_pane(cx);
        let split = self.active_split();

        split.update(cx, |split, cx| {
            split.split_active(direction, new_pane, window, cx);
            split.focus_active(window, cx);
        });

        cx.notify();
    }

    fn new_tab(&mut self, cx: &mut Context<Self>) {
        if self.tabs.len() >= MAX_SELECTABLE_TABS {
            return;
        }

        let tab = Self::create_tab(self.config.clone(), cx);
        self.tabs.push(tab);
        self.active_tab_index = self.tabs.len() - 1;
        self.needs_focus = true;
        cx.notify();
    }

    fn activate_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }

        self.active_tab_index = index;
        self.needs_focus = true;
        cx.notify();
    }

    fn focus_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let split = self.active_split();
        split.update(cx, |split, cx| split.focus_active(window, cx));
    }

    fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let split = self.active_split();
        let pane_count = split.read(cx).pane_count();

        if pane_count > 1 {
            let pane_to_focus = split.update(cx, |split, cx| split.remove_active_pane(window, cx));
            if let Some(pane) = pane_to_focus {
                pane.update(cx, |pane, cx| pane.request_focus(window, cx));
            }
            cx.notify();
            return;
        }

        if self.tabs.len() <= 1 {
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

    fn check_for_exits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut focus_after_remove = None;
        let active_split = self.active_split();

        for tab in &self.tabs {
            let pane_count = tab.split.read(cx).pane_count();
            if pane_count <= 1 {
                continue;
            }

            let panes = tab.split.read(cx).panes();
            let exited_panes: Vec<_> = panes
                .iter()
                .filter_map(|pane| {
                    let exited = pane.update(cx, |pane, cx| pane.check_exit(cx));
                    exited.then_some(pane.entity_id())
                })
                .collect();

            for pane_id in exited_panes {
                let pane_to_focus = tab
                    .split
                    .update(cx, |split, cx| split.remove_pane_by_id(pane_id, window, cx));
                if tab.split == active_split {
                    focus_after_remove = pane_to_focus;
                }
            }
        }

        if let Some(pane) = focus_after_remove {
            pane.update(cx, |pane, cx| pane.request_focus(window, cx));
            cx.notify();
        }
    }

    fn on_split_right(
        &mut self,
        _action: &SplitRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_active(SplitDirection::Row, window, cx);
    }

    fn on_split_down(&mut self, _action: &SplitDown, window: &mut Window, cx: &mut Context<Self>) {
        self.split_active(SplitDirection::Column, window, cx);
    }

    fn on_new_tab(&mut self, _action: &NewTab, _window: &mut Window, cx: &mut Context<Self>) {
        self.new_tab(cx);
    }

    fn on_close_active(
        &mut self,
        _action: &CloseActive,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_active(window, cx);
    }

    fn on_toggle_sidebar(
        &mut self,
        _action: &ToggleSidebar,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_sidebar(cx);
    }

    fn on_select_tab(&mut self, action: &SelectTab, _window: &mut Window, cx: &mut Context<Self>) {
        self.activate_tab(action.index, cx);
    }
}

pub fn shortcut_action(keystroke: &gpui::Keystroke) -> Option<Box<dyn gpui::Action>> {
    let modifiers = keystroke.modifiers;
    let key = keystroke.key.as_str();

    if modifiers.alt && !modifiers.control && !modifiers.platform && key == "enter" {
        return if modifiers.shift {
            Some(Box::new(SplitDown))
        } else {
            Some(Box::new(SplitRight))
        };
    }

    if !modifiers.control || modifiers.alt || modifiers.platform || modifiers.shift {
        return None;
    }

    match key {
        "t" => Some(Box::new(NewTab)),
        "d" => Some(Box::new(CloseActive)),
        "b" => Some(Box::new(ToggleSidebar)),
        "1" => Some(Box::new(SelectTab { index: 0 })),
        "2" => Some(Box::new(SelectTab { index: 1 })),
        "3" => Some(Box::new(SelectTab { index: 2 })),
        "4" => Some(Box::new(SelectTab { index: 3 })),
        "5" => Some(Box::new(SelectTab { index: 4 })),
        "6" => Some(Box::new(SelectTab { index: 5 })),
        "7" => Some(Box::new(SelectTab { index: 6 })),
        "8" => Some(Box::new(SelectTab { index: 7 })),
        "9" => Some(Box::new(SelectTab { index: 8 })),
        _ => None,
    }
}

fn short_current_dir_title() -> String {
    let Ok(current_dir) = std::env::current_dir() else {
        return "term".to_string();
    };

    if let Some(home) = user_home_dir()
        && current_dir == home
    {
        return "~".to_string();
    }

    current_dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("term")
        .to_string()
}

fn user_home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let drive = std::env::var_os("HOMEDRIVE")?;
            let path = std::env::var_os("HOMEPATH")?;
            Some(Path::new(&drive).join(path))
        })
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
        self.check_for_exits(window, cx);
        if self.needs_focus {
            self.needs_focus = false;
            self.focus_active_tab(window, cx);
        }

        let maximize_button = if window.is_maximized() {
            WindowsCaptionButton::Restore
        } else {
            WindowsCaptionButton::Maximize
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::rgb(WINDOW_BACKGROUND))
            .on_action(cx.listener(Self::on_split_right))
            .on_action(cx.listener(Self::on_split_down))
            .on_action(cx.listener(Self::on_new_tab))
            .on_action(cx.listener(Self::on_close_active))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_select_tab))
            .child(
                div()
                    .h(px(TITLE_BAR_HEIGHT_PX))
                    .flex()
                    .flex_row()
                    .items_center()
                    .bg(gpui::rgb(WINDOW_BACKGROUND))
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
                    )),
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
    }
}

impl PaneContainer {
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
                let label = (index + 1).to_string();
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
