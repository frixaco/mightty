use gpui::{
    AnyElement, Context, Entity, EntityId, IntoElement, Render, Window, div, prelude::*, px,
};

use crate::pane::Pane;

const SEPARATOR_COLOR: u32 = 0x00c853;
const SEPARATOR_SIZE_PX: f32 = 1.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Row,
    Column,
}

enum SplitNode {
    Pane(Entity<Pane>),
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
    pub fn with_pane(pane: Entity<Pane>) -> Self {
        Self {
            active_pane_id: pane.entity_id(),
            root: SplitNode::Pane(pane),
        }
    }

    pub fn pane_count(&self) -> usize {
        self.root.pane_count()
    }

    pub fn panes(&self) -> Vec<Entity<Pane>> {
        let mut panes = Vec::new();
        self.root.collect_panes(&mut panes);
        panes
    }

    pub fn split_active(
        &mut self,
        direction: SplitDirection,
        pane: Entity<Pane>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.update_active_from_focus(window, cx);

        let target_id = self.active_pane_id;
        let new_pane_id = pane.entity_id();
        if self.root.split_pane(target_id, direction, pane) {
            self.active_pane_id = new_pane_id;
        }
    }

    pub fn remove_active_pane(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Pane>> {
        self.update_active_from_focus(window, cx);
        self.remove_pane_and_select_next(self.active_pane_id)
    }

    pub fn remove_pane_by_id(
        &mut self,
        pane_id: EntityId,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Pane>> {
        self.update_active_from_focus(window, cx);
        self.remove_pane_and_select_next(pane_id)
    }

    fn remove_pane_and_select_next(&mut self, pane_id: EntityId) -> Option<Entity<Pane>> {
        if self.pane_count() <= 1 {
            return None;
        }

        let panes_before = self.panes();
        let target_index = panes_before
            .iter()
            .position(|pane| pane.entity_id() == pane_id)
            .unwrap_or(0);

        if !self.root.remove_pane(pane_id) {
            return None;
        }
        self.root.collapse_single_child_splits();

        let panes_after = self.panes();
        let focus_index = target_index.min(panes_after.len().saturating_sub(1));
        let pane_to_focus = panes_after.get(focus_index).cloned();
        if let Some(pane) = &pane_to_focus {
            self.active_pane_id = pane.entity_id();
        }

        pane_to_focus
    }

    pub fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panes = self.panes();
        let pane = panes
            .iter()
            .find(|pane| pane.entity_id() == self.active_pane_id)
            .cloned()
            .or_else(|| panes.first().cloned());

        if let Some(pane) = pane {
            self.active_pane_id = pane.entity_id();
            pane.update(cx, |pane, cx| pane.request_focus(window, cx));
        }
    }

    fn update_active_from_focus(&mut self, window: &Window, cx: &Context<Self>) {
        if let Some(focused_pane_id) = self.root.focused_pane_id(window, cx) {
            self.active_pane_id = focused_pane_id;
        }
    }
}

impl SplitNode {
    fn pane_count(&self) -> usize {
        match self {
            Self::Pane(_) => 1,
            Self::Split { children, .. } => children.iter().map(Self::pane_count).sum(),
        }
    }

    fn collect_panes(&self, panes: &mut Vec<Entity<Pane>>) {
        match self {
            Self::Pane(pane) => panes.push(pane.clone()),
            Self::Split { children, .. } => {
                for child in children {
                    child.collect_panes(panes);
                }
            }
        }
    }

    fn focused_pane_id(&self, window: &Window, cx: &Context<Split>) -> Option<EntityId> {
        match self {
            Self::Pane(pane) => pane
                .read(cx)
                .is_focused(window, cx)
                .then_some(pane.entity_id()),
            Self::Split { children, .. } => children
                .iter()
                .find_map(|child| child.focused_pane_id(window, cx)),
        }
    }

    fn split_pane(
        &mut self,
        target_id: EntityId,
        direction: SplitDirection,
        pane: Entity<Pane>,
    ) -> bool {
        match self {
            Self::Pane(existing_pane) if existing_pane.entity_id() == target_id => {
                let existing_pane = existing_pane.clone();
                *self = Self::Split {
                    direction,
                    children: vec![Self::Pane(existing_pane), Self::Pane(pane)],
                };
                true
            }
            Self::Pane(_) => false,
            Self::Split {
                direction: split_direction,
                children,
            } => {
                for index in 0..children.len() {
                    if let Self::Pane(existing_pane) = &children[index]
                        && existing_pane.entity_id() == target_id
                        && *split_direction == direction
                    {
                        children.insert(index + 1, Self::Pane(pane));
                        return true;
                    }

                    if children[index].split_pane(target_id, direction, pane.clone()) {
                        return true;
                    }
                }
                false
            }
        }
    }

    fn remove_pane(&mut self, target_id: EntityId) -> bool {
        match self {
            Self::Pane(_) => false,
            Self::Split { children, .. } => {
                let Some(index) = children.iter().position(
                    |child| matches!(child, Self::Pane(pane) if pane.entity_id() == target_id),
                ) else {
                    for child in children {
                        if child.remove_pane(target_id) {
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
            Self::Pane(pane) => div()
                .size_full()
                .flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(pane.clone())
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
