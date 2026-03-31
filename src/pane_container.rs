use gpui::{actions, div, prelude::*, Context, Entity, IntoElement, KeyBinding, Render, Window};

use crate::pane::Pane;
use crate::split::Split;
use crate::widget::TerminalConfig;

actions!(terminal, [SplitRight]);

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

impl Render for PaneContainer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.check_for_exits(cx);

        div()
            .size_full()
            .bg(gpui::rgba(0x1a1a1a))
            .on_action(cx.listener(Self::on_split_right))
            .child(self.split.clone())
    }
}
