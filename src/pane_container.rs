use gpui::{
    Context, Entity, Font, FontFallbacks, IntoElement, KeyBinding, Render, Window,
    WindowControlArea, actions, div, font, prelude::*, px,
};
use gpui_component::InteractiveElementExt;
use std::sync::OnceLock;

use crate::pane::Pane;
use crate::split::Split;
use crate::widget::TerminalConfig;

actions!(terminal, [SplitRight]);

const WINDOW_BACKGROUND: u32 = 0x000000;
const WINDOW_HORIZONTAL_PADDING_PX: f32 = 8.0;
const TITLE_BAR_HEIGHT_PX: f32 = 34.0;
const WINDOW_CONTROL_WIDTH_PX: f32 = 46.0;

#[derive(Clone, Copy)]
enum WindowsCaptionButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

pub struct PaneContainer {
    split: Entity<Split>,
    config: TerminalConfig,
}

impl PaneContainer {
    pub fn new(config: TerminalConfig, cx: &mut Context<Self>) -> Self {
        let pane = cx.new(|cx| Pane::new(config.clone(), cx));
        let split = cx.new(|_cx| Split::with_pane(pane));

        Self { split, config }
    }

    pub fn bind_keys(cx: &mut gpui::App) {
        cx.bind_keys([KeyBinding::new("alt-enter", SplitRight, None)]);
    }

    fn split_right(&mut self, cx: &mut Context<Self>) {
        let new_pane = cx.new(|cx| Pane::new(self.config.clone(), cx));

        self.split.update(cx, |split, _cx| {
            split.add_pane(new_pane.clone());
        });

        cx.notify();
    }

    fn check_for_exits(&mut self, cx: &mut Context<Self>) {
        let pane_count = self.split.read(cx).pane_count();
        if pane_count <= 1 {
            return;
        }

        let panes: Vec<_> = self.split.read(cx).panes().cloned().collect();
        let mut panes_to_remove: Vec<usize> = Vec::new();

        for (i, pane) in panes.iter().enumerate() {
            let exited = pane.update(cx, |pane, cx| pane.check_exit(cx));
            if exited {
                panes_to_remove.push(i);
            }
        }

        for idx in panes_to_remove.iter().rev() {
            self.remove_pane_at(*idx, cx);
        }
    }

    fn remove_pane_at(&mut self, index: usize, cx: &mut Context<Self>) {
        let pane_count = self.split.read(cx).pane_count();
        if pane_count <= 1 {
            return;
        }

        self.split.update(cx, |split, _cx| {
            split.remove_pane(index);
        });

        cx.notify();
    }

    fn on_split_right(
        &mut self,
        _action: &SplitRight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_right(cx);
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
        self.check_for_exits(cx);

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
                    .overflow_hidden()
                    .pl(px(WINDOW_HORIZONTAL_PADDING_PX))
                    .pr(px(WINDOW_HORIZONTAL_PADDING_PX))
                    .child(self.split.clone()),
            )
    }
}
