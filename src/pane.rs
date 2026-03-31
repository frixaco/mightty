use gpui::{div, prelude::*, Context, Entity, IntoElement, Render, Window};

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
}

impl Render for Pane {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex_1().child(self.terminal.clone())
    }
}
