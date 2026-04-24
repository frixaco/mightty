use gpui::{App, Context, Entity, IntoElement, Render, Window, div, prelude::*, px};

use crate::widget::{TerminalConfig, TerminalWidget};

pub struct Pane {
    pub terminal: Entity<TerminalWidget>,
}

impl Pane {
    pub fn new(config: TerminalConfig, cx: &mut Context<Self>) -> Self {
        let terminal = cx.new(|cx| TerminalWidget::new(config, cx));

        Self { terminal }
    }

    pub fn check_exit(&mut self, cx: &mut Context<Self>) -> bool {
        self.terminal
            .update(cx, |terminal, _cx| terminal.check_exit())
    }

    pub fn request_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, _cx| terminal.request_focus(window));
    }

    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.terminal.read(cx).focus_handle().is_focused(window)
    }
}

impl Render for Pane {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_1()
            .rounded(px(4.0))
            .overflow_hidden()
            .bg(gpui::rgb(0x000000))
            .child(self.terminal.clone())
    }
}
