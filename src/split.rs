use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*};

use crate::pane::Pane;

#[derive(Default)]
pub struct Split {
    panes: Vec<Entity<Pane>>,
}

impl Split {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_pane(pane: Entity<Pane>) -> Self {
        Self { panes: vec![pane] }
    }

    pub fn add_pane(&mut self, pane: Entity<Pane>) {
        self.panes.push(pane);
    }

    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    pub fn panes(&self) -> impl Iterator<Item = &Entity<Pane>> {
        self.panes.iter()
    }

    pub fn remove_pane(&mut self, index: usize) -> Option<Entity<Pane>> {
        if index < self.panes.len() {
            Some(self.panes.remove(index))
        } else {
            None
        }
    }
}

impl Render for Split {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if self.panes.is_empty() {
            return div().size_full().bg(gpui::rgba(0x1a1a1a));
        }

        div().size_full().flex().flex_row().gap_0().children(
            self.panes
                .iter()
                .map(|pane| div().flex().flex_grow().h_full().child(pane.clone())),
        )
    }
}
