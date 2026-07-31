//! Binary pane topology, pure layout geometry, and GPUI pane rendering.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{
    AnyElement, Bounds, Context, DragMoveEvent, Empty, Entity, EntityId, IntoElement, Pixels,
    Render, Window, canvas, div, prelude::*, px,
};
use serde::{Deserialize, Serialize};

use crate::action::{ActionBinding, Direction};
use crate::profile::ProfileId;
use crate::widget::TerminalWidget;

const SEPARATOR_COLOR: u32 = 0x00c853;
const SEPARATOR_SIZE_PX: f32 = 4.0;
const MIN_PANE_WIDTH_PX: f32 = 80.0;
const MIN_PANE_HEIGHT_PX: f32 = 54.0;

static NEXT_PANE_ID: AtomicU64 = AtomicU64::new(1);

/// Stable identity for one pane in runtime and saved workspace topology.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaneId(u64);

impl PaneId {
    pub fn value(self) -> u64 {
        self.0
    }

    fn fresh(panes: &BTreeMap<PaneId, RuntimePane>) -> Self {
        loop {
            let id = Self(NEXT_PANE_ID.fetch_add(1, Ordering::Relaxed));
            if !panes.contains_key(&id) {
                return id;
            }
        }
    }

    #[cfg(test)]
    pub(crate) const fn test(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

/// Serializable split topology. Runtime terminal entities live in a separate map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SplitNode {
    Leaf {
        pane_id: PaneId,
    },
    Branch {
        axis: SplitAxis,
        ratio: f32,
        first: Box<SplitNode>,
        second: Box<SplitNode>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl LayoutRect {
    fn center(self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    fn overlap_x(self, other: Self) -> f32 {
        ((self.x + self.width).min(other.x + other.width) - self.x.max(other.x)).max(0.0)
    }

    fn overlap_y(self, other: Self) -> f32 {
        ((self.y + self.height).min(other.y + other.height) - self.y.max(other.y)).max(0.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneLayout {
    pub pane_id: PaneId,
    pub bounds: LayoutRect,
}

#[derive(Clone, Debug)]
struct DividerLayout {
    path: NodePath,
    axis: SplitAxis,
    branch_bounds: LayoutRect,
    bounds: LayoutRect,
}

#[derive(Clone, Debug, Default)]
struct SplitLayout {
    panes: Vec<PaneLayout>,
    dividers: Vec<DividerLayout>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct NodePath(Vec<bool>);

impl NodePath {
    fn first(&self) -> Self {
        let mut path = self.0.clone();
        path.push(false);
        Self(path)
    }

    fn second(&self) -> Self {
        let mut path = self.0.clone();
        path.push(true);
        Self(path)
    }
}

struct RuntimePane {
    terminal: Entity<TerminalWidget>,
    profile_id: ProfileId,
}

pub struct Split {
    root: SplitNode,
    panes: BTreeMap<PaneId, RuntimePane>,
    active_pane_id: PaneId,
    zoomed_pane_id: Option<PaneId>,
    layout_bounds: Option<Bounds<Pixels>>,
}

impl Split {
    pub fn with_terminal(terminal: Entity<TerminalWidget>, profile_id: ProfileId) -> Self {
        let pane_id = PaneId::fresh(&BTreeMap::new());
        let mut panes = BTreeMap::new();
        panes.insert(
            pane_id,
            RuntimePane {
                terminal,
                profile_id,
            },
        );
        Self {
            root: SplitNode::Leaf { pane_id },
            panes,
            active_pane_id: pane_id,
            zoomed_pane_id: None,
            layout_bounds: None,
        }
    }

    pub fn from_restored(
        root: SplitNode,
        panes: Vec<(PaneId, Entity<TerminalWidget>, ProfileId)>,
        active_pane_id: PaneId,
    ) -> Result<Self, String> {
        let panes = panes
            .into_iter()
            .map(|(pane_id, terminal, profile_id)| {
                (
                    pane_id,
                    RuntimePane {
                        terminal,
                        profile_id,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let topology_ids = root.pane_ids();
        let unique_topology_ids = topology_ids.iter().copied().collect::<BTreeSet<_>>();
        let runtime_ids = panes.keys().copied().collect::<BTreeSet<_>>();
        if topology_ids.len() != unique_topology_ids.len() || unique_topology_ids != runtime_ids {
            return Err("restored pane records do not match split topology".to_string());
        }
        if !panes.contains_key(&active_pane_id) {
            return Err("restored active pane does not exist".to_string());
        }
        Ok(Self {
            root,
            panes,
            active_pane_id,
            zoomed_pane_id: None,
            layout_bounds: None,
        })
    }

    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    pub fn topology(&self) -> &SplitNode {
        &self.root
    }

    pub fn pane_profiles(&self) -> impl Iterator<Item = (PaneId, &ProfileId)> {
        self.panes
            .iter()
            .map(|(pane_id, pane)| (*pane_id, &pane.profile_id))
    }

    pub fn pane_entities(
        &self,
    ) -> impl Iterator<Item = (PaneId, &ProfileId, Entity<TerminalWidget>)> {
        self.panes
            .iter()
            .map(|(pane_id, pane)| (*pane_id, &pane.profile_id, pane.terminal.clone()))
    }

    pub fn active_pane_id(&self) -> PaneId {
        self.active_pane_id
    }

    pub fn exited_terminal_ids(&self, cx: &gpui::App) -> Vec<EntityId> {
        self.panes
            .values()
            .filter(|pane| pane.terminal.read(cx).has_exited())
            .map(|pane| pane.terminal.entity_id())
            .collect()
    }

    pub fn pane_id_for_entity(&self, entity_id: EntityId) -> Option<PaneId> {
        self.panes.iter().find_map(|(pane_id, pane)| {
            (pane.terminal.entity_id() == entity_id).then_some(*pane_id)
        })
    }

    pub fn split_active(
        &mut self,
        axis: SplitAxis,
        terminal: Entity<TerminalWidget>,
        profile_id: ProfileId,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.update_active_from_focus(window, cx);
        let pane_id = PaneId::fresh(&self.panes);
        if !self.root.split_leaf(self.active_pane_id, axis, pane_id) {
            return;
        }
        self.panes.insert(
            pane_id,
            RuntimePane {
                terminal,
                profile_id,
            },
        );
        self.active_pane_id = pane_id;
        self.zoomed_pane_id = None;
    }

    pub fn remove_active_pane(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalWidget>> {
        self.update_active_from_focus(window, cx);
        self.remove_pane(self.active_pane_id)
    }

    pub fn remove_pane_by_entity(&mut self, entity_id: EntityId) -> Option<Entity<TerminalWidget>> {
        let pane_id = self.pane_id_for_entity(entity_id)?;
        self.remove_pane(pane_id)
    }

    fn remove_pane(&mut self, pane_id: PaneId) -> Option<Entity<TerminalWidget>> {
        if self.panes.len() <= 1 || !self.root.contains(pane_id) {
            return None;
        }
        let pane_order = self.root.pane_ids();
        let removed_index = pane_order
            .iter()
            .position(|candidate| *candidate == pane_id)
            .unwrap_or(0);
        self.root = self.root.clone().without(pane_id)?;
        self.panes.remove(&pane_id);
        let remaining = self.root.pane_ids();
        self.active_pane_id = remaining[removed_index.min(remaining.len() - 1)];
        if self.zoomed_pane_id == Some(pane_id) {
            self.zoomed_pane_id = None;
        }
        self.panes
            .get(&self.active_pane_id)
            .map(|pane| pane.terminal.clone())
    }

    pub fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(terminal) = self.active_terminal(window, cx) {
            terminal.update(cx, |terminal, _cx| terminal.request_focus(window));
        }
    }

    pub fn active_terminal(
        &mut self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<Entity<TerminalWidget>> {
        self.update_active_from_focus(window, cx);
        self.panes
            .get(&self.active_pane_id)
            .map(|pane| pane.terminal.clone())
    }

    pub fn focus_direction(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.update_active_from_focus(window, cx);
        let layout = self.current_layout(window);
        let Some(next) = directional_neighbor(&layout.panes, self.active_pane_id, direction) else {
            return false;
        };
        self.active_pane_id = next;
        self.zoomed_pane_id = None;
        self.focus_active(window, cx);
        true
    }

    pub fn resize_active(
        &mut self,
        direction: Direction,
        amount: u16,
        window: &Window,
        cx: &Context<Self>,
    ) -> bool {
        self.update_active_from_focus(window, cx);
        let bounds = self.current_root_rect(window);
        resize_pane(
            &mut self.root,
            self.active_pane_id,
            direction,
            f32::from(amount),
            bounds,
        )
    }

    pub fn toggle_zoom(&mut self, window: &Window, cx: &Context<Self>) {
        self.update_active_from_focus(window, cx);
        self.zoomed_pane_id = if self.zoomed_pane_id == Some(self.active_pane_id) {
            None
        } else {
            Some(self.active_pane_id)
        };
    }

    pub fn set_action_bindings(&self, bindings: &[ActionBinding], cx: &mut Context<Self>) {
        for pane in self.panes.values() {
            pane.terminal.update(cx, |terminal, _cx| {
                terminal.set_action_bindings(bindings.to_vec())
            });
        }
    }

    fn update_active_from_focus(&mut self, window: &Window, cx: &Context<Self>) {
        if let Some((pane_id, _)) = self
            .panes
            .iter()
            .find(|(_, pane)| pane.terminal.read(cx).focus_handle().is_focused(window))
        {
            self.active_pane_id = *pane_id;
        }
    }

    fn current_root_rect(&self, window: &Window) -> LayoutRect {
        let size = self
            .layout_bounds
            .map_or_else(|| window.viewport_size(), |bounds| bounds.size);
        LayoutRect {
            x: 0.0,
            y: 0.0,
            width: size.width.into(),
            height: size.height.into(),
        }
    }

    fn current_layout(&self, window: &Window) -> SplitLayout {
        layout_tree(&self.root, self.current_root_rect(window))
    }

    fn update_layout_bounds(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.layout_bounds != Some(bounds) {
            self.layout_bounds = Some(bounds);
            cx.notify();
        }
    }

    fn drag_divider(
        &mut self,
        path: &NodePath,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(root_bounds) = self.layout_bounds else {
            return;
        };
        let layout = layout_tree(
            &self.root,
            LayoutRect {
                x: 0.0,
                y: 0.0,
                width: root_bounds.size.width.into(),
                height: root_bounds.size.height.into(),
            },
        );
        let Some(divider) = layout.dividers.iter().find(|divider| &divider.path == path) else {
            return;
        };
        let local_x: f32 = (position.x - root_bounds.origin.x).into();
        let local_y: f32 = (position.y - root_bounds.origin.y).into();
        let ratio = match divider.axis {
            SplitAxis::Horizontal => {
                (local_x - divider.branch_bounds.x) / divider.branch_bounds.width
            }
            SplitAxis::Vertical => {
                (local_y - divider.branch_bounds.y) / divider.branch_bounds.height
            }
        };
        if self.root.set_ratio(path, ratio) {
            cx.notify();
        }
    }

    fn on_divider_drag_move(
        &mut self,
        event: &DragMoveEvent<DividerDrag>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (owner, path) = {
            let drag = event.drag(cx);
            (drag.owner, drag.path.clone())
        };
        if owner == cx.entity_id() {
            self.drag_divider(&path, event.event.position, cx);
        }
    }

    fn render_layout(&self, layout: SplitLayout, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut elements = Vec::new();
        let zoomed = self.zoomed_pane_id;
        for pane_layout in layout.panes {
            if zoomed.is_some_and(|pane_id| pane_id != pane_layout.pane_id) {
                continue;
            }
            let Some(pane) = self.panes.get(&pane_layout.pane_id) else {
                continue;
            };
            let bounds = if zoomed.is_some() {
                self.layout_bounds
                    .map_or(pane_layout.bounds, |bounds| LayoutRect {
                        x: 0.0,
                        y: 0.0,
                        width: bounds.size.width.into(),
                        height: bounds.size.height.into(),
                    })
            } else {
                pane_layout.bounds
            };
            elements.push(
                div()
                    .absolute()
                    .left(px(bounds.x))
                    .top(px(bounds.y))
                    .w(px(bounds.width))
                    .h(px(bounds.height))
                    .min_w_0()
                    .min_h_0()
                    .rounded(px(4.0))
                    .overflow_hidden()
                    .bg(gpui::rgb(0x000000))
                    .child(pane.terminal.clone())
                    .into_any_element(),
            );
        }
        if zoomed.is_none() {
            for (index, divider) in layout.dividers.into_iter().enumerate() {
                let is_horizontal = divider.axis == SplitAxis::Horizontal;
                let drag = DividerDrag {
                    owner: cx.entity_id(),
                    path: divider.path.clone(),
                };
                elements.push(
                    div()
                        .id(("split-divider", index))
                        .absolute()
                        .left(px(divider.bounds.x))
                        .top(px(divider.bounds.y))
                        .w(px(divider.bounds.width))
                        .h(px(divider.bounds.height))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(is_horizontal, |divider| divider.cursor_col_resize())
                        .when(!is_horizontal, |divider| divider.cursor_row_resize())
                        .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                        .child(div().size_full().bg(gpui::rgb(SEPARATOR_COLOR)))
                        .into_any_element(),
                );
            }
        }
        elements
    }
}

#[derive(Clone)]
struct DividerDrag {
    owner: EntityId,
    path: NodePath,
}

impl Render for DividerDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

impl SplitNode {
    fn contains(&self, pane_id: PaneId) -> bool {
        match self {
            Self::Leaf { pane_id: candidate } => *candidate == pane_id,
            Self::Branch { first, second, .. } => {
                first.contains(pane_id) || second.contains(pane_id)
            }
        }
    }

    pub(crate) fn pane_ids(&self) -> Vec<PaneId> {
        let mut pane_ids = Vec::new();
        self.collect_pane_ids(&mut pane_ids);
        pane_ids
    }

    fn collect_pane_ids(&self, pane_ids: &mut Vec<PaneId>) {
        match self {
            Self::Leaf { pane_id } => pane_ids.push(*pane_id),
            Self::Branch { first, second, .. } => {
                first.collect_pane_ids(pane_ids);
                second.collect_pane_ids(pane_ids);
            }
        }
    }

    fn split_leaf(&mut self, target: PaneId, axis: SplitAxis, new_pane_id: PaneId) -> bool {
        match self {
            Self::Leaf { pane_id } if *pane_id == target => {
                *self = Self::Branch {
                    axis,
                    ratio: 0.5,
                    first: Box::new(Self::Leaf { pane_id: target }),
                    second: Box::new(Self::Leaf {
                        pane_id: new_pane_id,
                    }),
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Branch { first, second, .. } => {
                first.split_leaf(target, axis, new_pane_id)
                    || second.split_leaf(target, axis, new_pane_id)
            }
        }
    }

    fn without(self, target: PaneId) -> Option<Self> {
        match self {
            Self::Leaf { pane_id } => (pane_id != target).then_some(Self::Leaf { pane_id }),
            Self::Branch {
                axis,
                ratio,
                first,
                second,
            } => {
                let first = first.without(target);
                let second = second.without(target);
                match (first, second) {
                    (Some(first), Some(second)) => Some(Self::Branch {
                        axis,
                        ratio,
                        first: Box::new(first),
                        second: Box::new(second),
                    }),
                    (Some(child), None) | (None, Some(child)) => Some(child),
                    (None, None) => None,
                }
            }
        }
    }

    fn set_ratio(&mut self, path: &NodePath, ratio: f32) -> bool {
        let Some((last, parents)) = path.0.split_last() else {
            if let Self::Branch { ratio: current, .. } = self {
                *current = ratio.clamp(0.05, 0.95);
                return true;
            }
            return false;
        };
        let mut node = self;
        for second in parents {
            let Self::Branch {
                first,
                second: second_node,
                ..
            } = node
            else {
                return false;
            };
            node = if *second { second_node } else { first };
        }
        let Self::Branch {
            first,
            second: second_node,
            ..
        } = node
        else {
            return false;
        };
        let child = if *last { second_node } else { first };
        if let Self::Branch { ratio: current, .. } = child.as_mut() {
            *current = ratio.clamp(0.05, 0.95);
            true
        } else {
            false
        }
    }
}

pub fn pane_layouts(root: &SplitNode, bounds: LayoutRect) -> Vec<PaneLayout> {
    layout_tree(root, bounds).panes
}

fn layout_tree(root: &SplitNode, bounds: LayoutRect) -> SplitLayout {
    let mut layout = SplitLayout::default();
    layout_node(root, bounds, &NodePath::default(), &mut layout);
    layout
}

fn layout_node(node: &SplitNode, bounds: LayoutRect, path: &NodePath, layout: &mut SplitLayout) {
    match node {
        SplitNode::Leaf { pane_id } => layout.panes.push(PaneLayout {
            pane_id: *pane_id,
            bounds,
        }),
        SplitNode::Branch {
            axis,
            ratio,
            first,
            second,
        } => {
            let ratio = clamped_ratio(node, bounds, *ratio);
            match axis {
                SplitAxis::Horizontal => {
                    let available = (bounds.width - SEPARATOR_SIZE_PX).max(0.0);
                    let first_width = available * ratio;
                    let second_width = available - first_width;
                    let first_bounds = LayoutRect {
                        width: first_width,
                        ..bounds
                    };
                    let divider_bounds = LayoutRect {
                        x: bounds.x + first_width,
                        width: SEPARATOR_SIZE_PX,
                        ..bounds
                    };
                    let second_bounds = LayoutRect {
                        x: divider_bounds.x + divider_bounds.width,
                        width: second_width,
                        ..bounds
                    };
                    layout_node(first, first_bounds, &path.first(), layout);
                    layout.dividers.push(DividerLayout {
                        path: path.clone(),
                        axis: *axis,
                        branch_bounds: bounds,
                        bounds: divider_bounds,
                    });
                    layout_node(second, second_bounds, &path.second(), layout);
                }
                SplitAxis::Vertical => {
                    let available = (bounds.height - SEPARATOR_SIZE_PX).max(0.0);
                    let first_height = available * ratio;
                    let second_height = available - first_height;
                    let first_bounds = LayoutRect {
                        height: first_height,
                        ..bounds
                    };
                    let divider_bounds = LayoutRect {
                        y: bounds.y + first_height,
                        height: SEPARATOR_SIZE_PX,
                        ..bounds
                    };
                    let second_bounds = LayoutRect {
                        y: divider_bounds.y + divider_bounds.height,
                        height: second_height,
                        ..bounds
                    };
                    layout_node(first, first_bounds, &path.first(), layout);
                    layout.dividers.push(DividerLayout {
                        path: path.clone(),
                        axis: *axis,
                        branch_bounds: bounds,
                        bounds: divider_bounds,
                    });
                    layout_node(second, second_bounds, &path.second(), layout);
                }
            }
        }
    }
}

fn clamped_ratio(node: &SplitNode, bounds: LayoutRect, ratio: f32) -> f32 {
    let SplitNode::Branch {
        axis,
        first,
        second,
        ..
    } = node
    else {
        return ratio;
    };
    let available = match axis {
        SplitAxis::Horizontal => bounds.width - SEPARATOR_SIZE_PX,
        SplitAxis::Vertical => bounds.height - SEPARATOR_SIZE_PX,
    }
    .max(1.0);
    let first_min = minimum_extent(first, *axis);
    let second_min = minimum_extent(second, *axis);
    let lower = (first_min / available).min(0.5);
    let upper = (1.0 - second_min / available).max(0.5);
    ratio.clamp(lower, upper)
}

fn minimum_extent(node: &SplitNode, requested_axis: SplitAxis) -> f32 {
    match node {
        SplitNode::Leaf { .. } => match requested_axis {
            SplitAxis::Horizontal => MIN_PANE_WIDTH_PX,
            SplitAxis::Vertical => MIN_PANE_HEIGHT_PX,
        },
        SplitNode::Branch {
            axis,
            first,
            second,
            ..
        } if *axis == requested_axis => {
            minimum_extent(first, requested_axis)
                + SEPARATOR_SIZE_PX
                + minimum_extent(second, requested_axis)
        }
        SplitNode::Branch { first, second, .. } => {
            minimum_extent(first, requested_axis).max(minimum_extent(second, requested_axis))
        }
    }
}

fn directional_neighbor(
    panes: &[PaneLayout],
    active_pane_id: PaneId,
    direction: Direction,
) -> Option<PaneId> {
    let active = panes
        .iter()
        .find(|pane| pane.pane_id == active_pane_id)?
        .bounds;
    let (active_x, active_y) = active.center();

    panes
        .iter()
        .filter(|pane| pane.pane_id != active_pane_id)
        .filter_map(|pane| {
            let (x, y) = pane.bounds.center();
            let (primary_distance, perpendicular_distance, overlap) = match direction {
                Direction::Left if x < active_x => (
                    active_x - x,
                    (active_y - y).abs(),
                    active.overlap_y(pane.bounds),
                ),
                Direction::Right if x > active_x => (
                    x - active_x,
                    (active_y - y).abs(),
                    active.overlap_y(pane.bounds),
                ),
                Direction::Up if y < active_y => (
                    active_y - y,
                    (active_x - x).abs(),
                    active.overlap_x(pane.bounds),
                ),
                Direction::Down if y > active_y => (
                    y - active_y,
                    (active_x - x).abs(),
                    active.overlap_x(pane.bounds),
                ),
                _ => return None,
            };
            Some((
                pane.pane_id,
                overlap <= 0.0,
                primary_distance,
                perpendicular_distance,
            ))
        })
        .min_by(|left, right| {
            left.1
                .cmp(&right.1)
                .then_with(|| left.2.total_cmp(&right.2))
                .then_with(|| left.3.total_cmp(&right.3))
                .then_with(|| left.0.cmp(&right.0))
        })
        .map(|candidate| candidate.0)
}

fn resize_pane(
    root: &mut SplitNode,
    pane_id: PaneId,
    direction: Direction,
    amount: f32,
    bounds: LayoutRect,
) -> bool {
    let Some((path, grows_first, axis)) = resize_branch(root, pane_id, direction) else {
        return false;
    };
    let layout = layout_tree(root, bounds);
    let Some(divider) = layout.dividers.iter().find(|divider| divider.path == path) else {
        return false;
    };
    let extent = match axis {
        SplitAxis::Horizontal => divider.branch_bounds.width,
        SplitAxis::Vertical => divider.branch_bounds.height,
    }
    .max(1.0);
    let current = ratio_at_path(root, &path).unwrap_or(0.5);
    let delta = amount
        * if axis == SplitAxis::Horizontal {
            8.0
        } else {
            18.0
        }
        / extent;
    root.set_ratio(&path, current + if grows_first { delta } else { -delta })
}

fn resize_branch(
    node: &SplitNode,
    pane_id: PaneId,
    direction: Direction,
) -> Option<(NodePath, bool, SplitAxis)> {
    fn visit(
        node: &SplitNode,
        pane_id: PaneId,
        direction: Direction,
        path: &NodePath,
    ) -> Option<(NodePath, bool, SplitAxis)> {
        let SplitNode::Branch {
            axis,
            first,
            second,
            ..
        } = node
        else {
            return None;
        };
        if let Some(found) = if first.contains(pane_id) {
            visit(first, pane_id, direction, &path.first())
        } else {
            visit(second, pane_id, direction, &path.second())
        } {
            return Some(found);
        }
        match (
            direction,
            axis,
            first.contains(pane_id),
            second.contains(pane_id),
        ) {
            (Direction::Right, SplitAxis::Horizontal, true, _)
            | (Direction::Down, SplitAxis::Vertical, true, _) => Some((path.clone(), true, *axis)),
            (Direction::Left, SplitAxis::Horizontal, _, true)
            | (Direction::Up, SplitAxis::Vertical, _, true) => Some((path.clone(), false, *axis)),
            _ => None,
        }
    }

    visit(node, pane_id, direction, &NodePath::default())
}

fn ratio_at_path(root: &SplitNode, path: &NodePath) -> Option<f32> {
    let mut node = root;
    for second in &path.0 {
        let SplitNode::Branch {
            first,
            second: second_node,
            ..
        } = node
        else {
            return None;
        };
        node = if *second { second_node } else { first };
    }
    match node {
        SplitNode::Branch { ratio, .. } => Some(*ratio),
        SplitNode::Leaf { .. } => None,
    }
}

impl Render for Split {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = self.current_layout(window);
        let elements = self.render_layout(layout, cx);
        let entity = cx.entity();

        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(gpui::rgb(0x000000))
            .on_drag_move(cx.listener(Self::on_divider_drag_move))
            .children(elements)
            .child(
                canvas(
                    move |bounds, _window, cx| {
                        entity.update(cx, |split, cx| {
                            split.update_layout_bounds(bounds, cx);
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(value: u64) -> SplitNode {
        SplitNode::Leaf {
            pane_id: PaneId::test(value),
        }
    }

    fn branch(axis: SplitAxis, ratio: f32, first: SplitNode, second: SplitNode) -> SplitNode {
        SplitNode::Branch {
            axis,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    #[test]
    fn split_tree_is_binary_and_close_collapses_the_parent() {
        let mut root = leaf(1);
        assert!(root.split_leaf(PaneId::test(1), SplitAxis::Horizontal, PaneId::test(2)));
        assert!(root.split_leaf(PaneId::test(2), SplitAxis::Vertical, PaneId::test(3)));
        assert_eq!(
            root.pane_ids(),
            [PaneId::test(1), PaneId::test(2), PaneId::test(3)]
        );

        let root = root.without(PaneId::test(2)).unwrap();
        assert_eq!(root.pane_ids(), [PaneId::test(1), PaneId::test(3)]);
        assert!(matches!(
            root,
            SplitNode::Branch {
                axis: SplitAxis::Horizontal,
                ..
            }
        ));
    }

    #[test]
    fn pure_layout_applies_ratios_and_minimum_pane_sizes() {
        let root = branch(SplitAxis::Horizontal, 0.95, leaf(1), leaf(2));
        let panes = pane_layouts(
            &root,
            LayoutRect {
                width: 400.0,
                height: 200.0,
                ..Default::default()
            },
        );

        assert_eq!(panes.len(), 2);
        assert!(panes[0].bounds.width > panes[1].bounds.width);
        assert!(panes[1].bounds.width >= MIN_PANE_WIDTH_PX);
        assert_eq!(
            panes[0].bounds.width + panes[1].bounds.width + SEPARATOR_SIZE_PX,
            400.0
        );
    }

    #[test]
    fn directional_focus_prefers_overlap_before_diagonal_distance() {
        let panes = vec![
            PaneLayout {
                pane_id: PaneId::test(1),
                bounds: LayoutRect {
                    width: 100.0,
                    height: 100.0,
                    ..Default::default()
                },
            },
            PaneLayout {
                pane_id: PaneId::test(2),
                bounds: LayoutRect {
                    x: 110.0,
                    width: 100.0,
                    height: 40.0,
                    ..Default::default()
                },
            },
            PaneLayout {
                pane_id: PaneId::test(3),
                bounds: LayoutRect {
                    x: 101.0,
                    y: 120.0,
                    width: 100.0,
                    height: 40.0,
                },
            },
        ];

        assert_eq!(
            directional_neighbor(&panes, PaneId::test(1), Direction::Right),
            Some(PaneId::test(2))
        );
    }

    #[test]
    fn resize_changes_only_the_nearest_matching_branch() {
        let mut root = branch(
            SplitAxis::Horizontal,
            0.5,
            leaf(1),
            branch(SplitAxis::Vertical, 0.5, leaf(2), leaf(3)),
        );
        assert!(resize_pane(
            &mut root,
            PaneId::test(2),
            Direction::Down,
            2.0,
            LayoutRect {
                width: 600.0,
                height: 400.0,
                ..Default::default()
            }
        ));

        let SplitNode::Branch { ratio, second, .. } = root else {
            panic!("root must remain a branch");
        };
        assert_eq!(ratio, 0.5);
        assert!(matches!(
            *second,
            SplitNode::Branch { ratio, .. } if ratio > 0.5
        ));
    }
}
