use gpui::{
    AnyElement, Context, Entity, EntityId, IntoElement, Render, Window, div, prelude::*, px,
};

use crate::action::ActionBinding;
use crate::widget::TerminalWidget;

const SEPARATOR_COLOR: u32 = 0x00c853;
const SEPARATOR_SIZE_PX: f32 = 1.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Row,
    Column,
}

enum SplitNode {
    Terminal(Entity<TerminalWidget>),
    Split {
        direction: SplitDirection,
        children: Vec<SplitNode>,
    },
}

pub struct Split {
    root: SplitNode,
    active_pane_id: EntityId,
}

impl Split {
    pub fn with_terminal(terminal: Entity<TerminalWidget>) -> Self {
        Self {
            active_pane_id: terminal.entity_id(),
            root: SplitNode::Terminal(terminal),
        }
    }

    pub fn pane_count(&self) -> usize {
        self.root.pane_count()
    }

    pub fn exited_terminal_ids(&self, cx: &gpui::App) -> Vec<EntityId> {
        let mut terminal_ids = Vec::new();
        self.root.collect_exited_terminal_ids(cx, &mut terminal_ids);
        terminal_ids
    }

    pub fn split_active(
        &mut self,
        direction: SplitDirection,
        terminal: Entity<TerminalWidget>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.update_active_from_focus(window, cx);

        let target_id = self.active_pane_id;
        let new_terminal_id = terminal.entity_id();
        if self.root.split_terminal(target_id, direction, terminal) {
            self.active_pane_id = new_terminal_id;
        }
    }

    pub fn remove_active_pane(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalWidget>> {
        self.update_active_from_focus(window, cx);
        self.remove_terminal_and_select_next(self.active_pane_id)
    }

    pub fn remove_pane_by_id(&mut self, pane_id: EntityId) -> Option<Entity<TerminalWidget>> {
        self.remove_terminal_and_select_next(pane_id)
    }

    fn remove_terminal_and_select_next(
        &mut self,
        pane_id: EntityId,
    ) -> Option<Entity<TerminalWidget>> {
        if self.pane_count() <= 1 {
            return None;
        }

        let target_index = self.root.terminal_index(pane_id).unwrap_or(0);

        if !self.root.remove_terminal(pane_id) {
            return None;
        }
        self.root.collapse_single_child_splits();

        let terminal_to_focus = self.root.terminal_at(target_index);
        if let Some(terminal) = &terminal_to_focus {
            self.active_pane_id = terminal.entity_id();
        }

        terminal_to_focus
    }

    pub fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self
            .root
            .terminal_by_id(self.active_pane_id)
            .or_else(|| self.root.terminal_at(0));

        if let Some(terminal) = terminal {
            self.active_pane_id = terminal.entity_id();
            terminal.update(cx, |terminal, _cx| terminal.request_focus(window));
        }
    }

    pub fn active_terminal(
        &mut self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<Entity<TerminalWidget>> {
        self.update_active_from_focus(window, cx);
        self.root
            .terminal_by_id(self.active_pane_id)
            .or_else(|| self.root.terminal_at(0))
    }

    pub fn set_action_bindings(&self, bindings: &[ActionBinding], cx: &mut Context<Self>) {
        self.root.set_action_bindings(bindings, cx);
    }

    fn update_active_from_focus(&mut self, window: &Window, cx: &Context<Self>) {
        if let Some(focused_pane_id) = self.root.focused_pane_id(window, cx) {
            self.active_pane_id = focused_pane_id;
        }
    }
}

impl SplitNode {
    fn set_action_bindings(&self, bindings: &[ActionBinding], cx: &mut Context<Split>) {
        match self {
            Self::Terminal(terminal) => {
                terminal.update(cx, |terminal, _cx| {
                    terminal.set_action_bindings(bindings.to_vec())
                });
            }
            Self::Split { children, .. } => {
                for child in children {
                    child.set_action_bindings(bindings, cx);
                }
            }
        }
    }

    fn pane_count(&self) -> usize {
        match self {
            Self::Terminal(_) => 1,
            Self::Split { children, .. } => children.iter().map(Self::pane_count).sum(),
        }
    }

    fn collect_exited_terminal_ids(&self, cx: &gpui::App, terminal_ids: &mut Vec<EntityId>) {
        match self {
            Self::Terminal(terminal) => {
                if terminal.read(cx).has_exited() {
                    terminal_ids.push(terminal.entity_id());
                }
            }
            Self::Split { children, .. } => {
                for child in children {
                    child.collect_exited_terminal_ids(cx, terminal_ids);
                }
            }
        }
    }

    fn focused_pane_id(&self, window: &Window, cx: &Context<Split>) -> Option<EntityId> {
        match self {
            Self::Terminal(terminal) => terminal
                .read(cx)
                .focus_handle()
                .is_focused(window)
                .then_some(terminal.entity_id()),
            Self::Split { children, .. } => children
                .iter()
                .find_map(|child| child.focused_pane_id(window, cx)),
        }
    }

    fn split_terminal(
        &mut self,
        target_id: EntityId,
        direction: SplitDirection,
        terminal: Entity<TerminalWidget>,
    ) -> bool {
        match self {
            Self::Terminal(existing_terminal) if existing_terminal.entity_id() == target_id => {
                let existing_terminal = existing_terminal.clone();
                *self = Self::Split {
                    direction,
                    children: vec![Self::Terminal(existing_terminal), Self::Terminal(terminal)],
                };
                true
            }
            Self::Terminal(_) => false,
            Self::Split { children, .. } => {
                for child in children {
                    if child.split_terminal(target_id, direction, terminal.clone()) {
                        return true;
                    }
                }
                false
            }
        }
    }

    fn remove_terminal(&mut self, target_id: EntityId) -> bool {
        match self {
            Self::Terminal(_) => false,
            Self::Split { children, .. } => {
                let Some(index) = children.iter().position(
                    |child| matches!(child, Self::Terminal(terminal) if terminal.entity_id() == target_id),
                ) else {
                    for child in children {
                        if child.remove_terminal(target_id) {
                            return true;
                        }
                    }
                    return false;
                };

                children.remove(index);
                true
            }
        }
    }

    fn terminal_by_id(&self, target_id: EntityId) -> Option<Entity<TerminalWidget>> {
        match self {
            Self::Terminal(terminal) => {
                (terminal.entity_id() == target_id).then(|| terminal.clone())
            }
            Self::Split { children, .. } => children
                .iter()
                .find_map(|child| child.terminal_by_id(target_id)),
        }
    }

    fn terminal_index(&self, target_id: EntityId) -> Option<usize> {
        let mut index = 0;
        self.find_terminal_index(target_id, &mut index)
    }

    fn find_terminal_index(&self, target_id: EntityId, index: &mut usize) -> Option<usize> {
        match self {
            Self::Terminal(terminal) => {
                let current = *index;
                *index += 1;
                (terminal.entity_id() == target_id).then_some(current)
            }
            Self::Split { children, .. } => children
                .iter()
                .find_map(|child| child.find_terminal_index(target_id, index)),
        }
    }

    fn terminal_at(&self, target_index: usize) -> Option<Entity<TerminalWidget>> {
        let mut index = 0;
        self.find_terminal_at(target_index, &mut index)
    }

    fn find_terminal_at(
        &self,
        target_index: usize,
        index: &mut usize,
    ) -> Option<Entity<TerminalWidget>> {
        match self {
            Self::Terminal(terminal) => {
                let current = *index;
                *index += 1;
                (current == target_index).then(|| terminal.clone())
            }
            Self::Split { children, .. } => children
                .iter()
                .find_map(|child| child.find_terminal_at(target_index, index)),
        }
    }

    fn collapse_single_child_splits(&mut self) {
        if let Self::Split { children, .. } = self {
            for child in children.iter_mut() {
                child.collapse_single_child_splits();
            }

            if children.len() == 1 {
                *self = children.remove(0);
            }
        }
    }

    fn render(&self) -> AnyElement {
        match self {
            Self::Terminal(terminal) => div()
                .size_full()
                .flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .rounded(px(4.0))
                .overflow_hidden()
                .bg(gpui::rgb(0x000000))
                .child(terminal.clone())
                .into_any_element(),
            Self::Split {
                direction,
                children,
            } => {
                let is_row = *direction == SplitDirection::Row;
                let mut elements = Vec::new();

                for (index, child) in children.iter().enumerate() {
                    if index > 0 {
                        elements.push(separator(*direction));
                    }
                    elements.push(child.render());
                }

                div()
                    .size_full()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .when(is_row, |div| div.flex_row())
                    .when(!is_row, |div| div.flex_col())
                    .children(elements)
                    .into_any_element()
            }
        }
    }
}

fn separator(direction: SplitDirection) -> AnyElement {
    let is_row = direction == SplitDirection::Row;

    div()
        .flex_shrink_0()
        .bg(gpui::rgb(SEPARATOR_COLOR))
        .when(is_row, |div| div.w(px(SEPARATOR_SIZE_PX)).h_full())
        .when(!is_row, |div| div.h(px(SEPARATOR_SIZE_PX)).w_full())
        .into_any_element()
}

impl Render for Split {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(gpui::rgb(0x000000))
            .child(self.root.render())
    }
}
