//! System CPU, memory and GPU usage shown in the title bar.
//!
//! One app-wide sampler runs once a second on the background executor, and
//! only while a window showing the meters is visible: GPU engine queries also
//! fetch engine clocks from the driver, which can cost about a millisecond of
//! CPU per engine. A window that renders the meters reads the shared model,
//! so GPUI redraws only those windows when a sample lands; nothing else is
//! notified.

#[cfg(windows)]
mod windows;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::hover_card::HoverCard;
use gpui_kit::{
    Anchor, AnyElement, App, Context, Entity, EntityId, Global, Pixels, SharedString, Task, Window,
    div, point, prelude::*, px,
};

use crate::theme;

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
/// First sample after resuming, so restored windows do not show stale values for long.
#[cfg(windows)]
const RESUME_INTERVAL: Duration = Duration::from_millis(250);
/// Samples per sparkline: one per 2px of its width.
const HISTORY_LEN: usize = 18;
const GRAPH_WIDTH_PX: f32 = 34.0;
const SPARKLINE_HEIGHT_PX: f32 = 12.0;
const SPARKLINE_STROKE_PX: f32 = 1.2;
const BAR_HEIGHT_PX: f32 = 4.0;
const FONT_SIZE_PX: f32 = 11.0;
const METER_GAP_PX: f32 = 14.0;
const METER_INNER_GAP_PX: f32 = 6.0;
const PADDING_X_PX: f32 = 12.0;
/// Width estimates for choosing which meters fit: an upper bound for the
/// uppercase UI-font labels, and the monospace advance for values.
const LABEL_CHAR_PX: f32 = 7.5;
const VALUE_CHAR_PX: f32 = FONT_SIZE_PX * 0.6;
/// Percentages keep the width of "100%" so neighbours do not shift.
const PERCENT_CHARS: usize = 4;
const GIB: f64 = (1u64 << 30) as f64;
const POPUP_OPEN_DELAY: Duration = Duration::from_millis(300);
const POPUP_WIDTH_PX: f32 = 400.0;
const POPUP_PADDING_X_PX: f32 = 10.0;
const POPUP_ROW_HEIGHT_PX: f32 = 22.0;
const POPUP_COLUMN_WIDTH_PX: f32 = 44.0;
const POPUP_INDENT_PX: f32 = 14.0;
/// About a dozen rows; longer lists scroll.
const POPUP_LIST_MAX_HEIGHT_PX: f32 = 280.0;
/// Spaces row IDs so each tab's panes get distinct ones.
const MAX_PANES_PER_TAB: usize = 1000;

#[derive(Clone, Debug, Default)]
pub(crate) struct Sample {
    /// Busy share of all logical processors since the previous sample.
    pub cpu: Option<f32>,
    pub memory: Option<MemoryUsage>,
    pub gpu: Option<GpuUsage>,
    /// Usage of each requested process tree.
    pub trees: HashMap<ProcessRoot, TreeUsage>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MemoryUsage {
    /// Usable physical memory that is not available.
    pub used: u64,
    /// Physical memory the OS can use.
    pub usable: u64,
    /// Installed memory, including what firmware and devices reserve.
    pub installed: u64,
}

/// A pane's root process: its ID and creation time, since IDs are reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ProcessRoot {
    pub pid: u32,
    /// FILETIME, in 100ns units.
    pub created: u64,
}

/// Usage of a process and all its descendants.
#[derive(Clone, Debug, Default)]
pub(crate) struct TreeUsage {
    /// Share of all processors since the previous sample; `None` on the first.
    pub cpu: Option<f32>,
    /// Private working set, in bytes.
    pub memory: u64,
    /// Share of time each GPU engine spent on the tree; empty when unknown.
    pub engines: Vec<f32>,
    pub processes: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct GpuUsage {
    pub name: Arc<str>,
    /// Busiest engine's share of time since the previous sample.
    pub utilization: f32,
    /// Dedicated GPU memory in use; the total is zero when there is none.
    pub dedicated_used: u64,
    pub dedicated_total: u64,
}

pub(crate) struct SystemUsage {
    latest: Sample,
    cpu_history: VecDeque<f32>,
    gpu_history: VecDeque<f32>,
    /// Visible views showing the meters; sampling pauses while there are none.
    viewers: BTreeSet<EntityId>,
    /// Pane process trees each view wants, sampled only while requested.
    process_roots: BTreeMap<EntityId, Vec<ProcessRoot>>,
    wake: flume::Sender<()>,
    _sampling: Task<()>,
}

struct SharedSystemUsage(Entity<SystemUsage>);

impl Global for SharedSystemUsage {}

impl SystemUsage {
    /// The app-wide usage model, sampling from its first use. `None` where
    /// sampling is not implemented, and in unit tests.
    pub(crate) fn shared(cx: &mut App) -> Option<Entity<Self>> {
        if !cfg!(all(windows, not(test))) {
            return None;
        }
        if let Some(shared) = cx.try_global::<SharedSystemUsage>() {
            return Some(shared.0.clone());
        }
        let (wake, wake_rx) = flume::bounded(1);
        let usage = cx.new(|cx| Self {
            latest: Sample::default(),
            cpu_history: VecDeque::with_capacity(HISTORY_LEN),
            gpu_history: VecDeque::with_capacity(HISTORY_LEN),
            viewers: BTreeSet::new(),
            process_roots: BTreeMap::new(),
            wake,
            _sampling: Self::start_sampling(wake_rx, cx),
        });
        cx.set_global(SharedSystemUsage(usage.clone()));
        Some(usage)
    }

    /// Record whether `viewer` currently shows the meters in a visible window.
    pub(crate) fn set_viewer(&mut self, viewer: EntityId, visible: bool) {
        if visible {
            if self.viewers.insert(viewer) && self.viewers.len() == 1 {
                // Full means a wake is already pending.
                let _ = self.wake.try_send(());
            }
        } else {
            self.viewers.remove(&viewer);
        }
    }

    /// Ask for usage of these process trees in the coming samples; an empty
    /// list withdraws the request.
    pub(crate) fn set_process_roots(&mut self, viewer: EntityId, roots: Vec<ProcessRoot>) {
        if roots.is_empty() {
            self.process_roots.remove(&viewer);
        } else {
            self.process_roots.insert(viewer, roots);
        }
    }

    fn requested_roots(&self) -> Vec<ProcessRoot> {
        let mut roots: Vec<_> = self.process_roots.values().flatten().copied().collect();
        roots.sort();
        roots.dedup();
        roots
    }

    #[cfg(windows)]
    fn start_sampling(wake: flume::Receiver<()>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                // Wait until a window shows the meters.
                loop {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    if !this.read_with(cx, |usage, _| usage.viewers.is_empty()) {
                        break;
                    }
                    if wake.recv_async().await.is_err() {
                        return;
                    }
                }
                // Fresh baselines, so the first sample does not average over the pause.
                let mut sampler = cx.background_spawn(async { windows::Sampler::new() }).await;
                let mut interval = RESUME_INTERVAL;
                loop {
                    cx.background_executor().timer(interval).await;
                    interval = SAMPLE_INTERVAL;
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    let (paused, roots) = this.read_with(cx, |usage, _| {
                        (usage.viewers.is_empty(), usage.requested_roots())
                    });
                    if paused {
                        this.update(cx, |usage, _| usage.pause());
                        break;
                    }
                    let (returned, sample) = cx
                        .background_spawn(async move {
                            let sample = sampler.sample(&roots);
                            (sampler, sample)
                        })
                        .await;
                    sampler = returned;
                    // Trees sampled for the first time only have baselines; follow up soon.
                    if sample.trees.values().any(|tree| tree.cpu.is_none()) {
                        interval = RESUME_INTERVAL;
                    }
                    this.update(cx, |usage, cx| usage.record(sample, cx));
                }
            }
        })
    }

    #[cfg(not(windows))]
    fn start_sampling(_wake: flume::Receiver<()>, _cx: &mut Context<Self>) -> Task<()> {
        Task::ready(())
    }

    /// Graphs restart after a pause rather than join samples across it.
    fn pause(&mut self) {
        self.cpu_history.clear();
        self.gpu_history.clear();
    }

    fn record(&mut self, sample: Sample, cx: &mut Context<Self>) {
        push_history(&mut self.cpu_history, sample.cpu);
        push_history(
            &mut self.gpu_history,
            sample.gpu.as_ref().map(|gpu| gpu.utilization),
        );
        self.latest = sample;
        cx.notify();
    }

    /// The meters that fit in `available` width. Meters are kept in order
    /// (CPU, memory, GPU, VRAM), and graphs are dropped before meters.
    /// Hovering them opens per-pane usage for `tabs`; `on_open_change`
    /// reports when that popup opens or closes.
    pub(crate) fn render(
        &self,
        available: Pixels,
        tabs: Vec<TabProcesses>,
        on_open_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Option<AnyElement> {
        let meters = self.meters();
        let widths: Vec<_> = meters
            .iter()
            .map(|meter| (meter.width(true), meter.width(false)))
            .collect();
        let (count, graphs) = fit(&widths, available.as_f32())?;
        let popup = UsagePopup {
            details: meters
                .iter()
                .map(|meter| (meter.label, SharedString::from(meter.detail.clone())))
                .collect(),
            tabs: tabs.iter().map(|tab| self.tab_row(tab)).collect(),
            gpu: self.latest.gpu.is_some(),
        };
        let strip = div()
            .id("system-usage")
            .h_full()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(METER_GAP_PX))
            .px(px(PADDING_X_PX))
            .text_size(px(FONT_SIZE_PX))
            .text_color(gpui_kit::rgb(theme::GREY))
            .children(
                meters
                    .into_iter()
                    .take(count)
                    .map(|meter| meter.render(graphs)),
            );
        Some(
            HoverCard::new("system-usage-card")
                .anchor(Anchor::TopRight)
                .open_delay(POPUP_OPEN_DELAY)
                .trigger(strip)
                .content(move |_, _, _| popup.render())
                .on_open_change(on_open_change)
                .into_any_element(),
        )
    }

    fn tab_row(&self, tab: &TabProcesses) -> TabRow {
        let panes: Vec<_> = tab
            .panes
            .iter()
            .map(|pane| PaneRow {
                title: pane.title.clone(),
                usage: pane
                    .root
                    .and_then(|root| self.latest.trees.get(&root))
                    .cloned(),
            })
            .collect();
        let trees: Vec<_> = panes
            .iter()
            .filter_map(|pane| pane.usage.as_ref())
            .collect();
        let total = (!trees.is_empty()).then(|| TreeUsage {
            cpu: trees.iter().map(|tree| tree.cpu).sum(),
            memory: trees.iter().map(|tree| tree.memory).sum(),
            // An engine can run work from several panes at once, so add per engine.
            engines: (0..trees
                .iter()
                .map(|tree| tree.engines.len())
                .max()
                .unwrap_or(0))
                .map(|engine| {
                    trees
                        .iter()
                        .filter_map(|tree| tree.engines.get(engine))
                        .sum()
                })
                .collect(),
            processes: trees.iter().map(|tree| tree.processes).sum(),
        });
        TabRow {
            title: tab.title.clone(),
            active: tab.active,
            usage: total,
            panes: if panes.len() > 1 { panes } else { Vec::new() },
        }
    }

    fn meters(&self) -> Vec<Meter> {
        let mut meters = Vec::new();
        if let Some(cpu) = self.latest.cpu {
            meters.push(Meter {
                id: "usage-cpu",
                label: "CPU",
                graph: Graph::Sparkline(self.cpu_history.iter().copied().collect(), theme::BLUE),
                value: percent(cpu),
                suffix: None,
                detail: format!("{} of all logical processors", percent(cpu).trim()),
            });
        }
        if let Some(memory) = self.latest.memory {
            let mut detail = format!(
                "{} GiB in use of {} GiB usable",
                format_gib(memory.used),
                format_gib(memory.usable)
            );
            if memory.installed > memory.usable {
                detail.push_str(&format!(
                    " \u{b7} {} GiB installed",
                    format_capacity(memory.installed)
                ));
            }
            meters.push(Meter {
                id: "usage-memory",
                label: "MEM",
                graph: Graph::Bar(
                    memory.used as f32 / memory.usable.max(1) as f32,
                    theme::PURPLE,
                ),
                value: format_gib(memory.used),
                suffix: Some(format!("/{}G", format_capacity(memory.installed))),
                detail,
            });
        }
        if let Some(gpu) = &self.latest.gpu {
            meters.push(Meter {
                id: "usage-gpu",
                label: "GPU",
                graph: Graph::Sparkline(self.gpu_history.iter().copied().collect(), theme::CYAN),
                value: percent(gpu.utilization),
                suffix: None,
                detail: format!(
                    "{} on the busiest engine \u{b7} {}",
                    percent(gpu.utilization).trim(),
                    gpu.name
                ),
            });
            if gpu.dedicated_total > 0 {
                meters.push(Meter {
                    id: "usage-vram",
                    label: "VRAM",
                    graph: Graph::None,
                    value: format_gib(gpu.dedicated_used),
                    suffix: Some(format!("/{}G", format_capacity(gpu.dedicated_total))),
                    detail: format!(
                        "{} GiB of {} GiB dedicated GPU memory in use",
                        format_gib(gpu.dedicated_used),
                        format_gib(gpu.dedicated_total)
                    ),
                });
            }
        }
        meters
    }
}

/// A tab and its panes, in sidebar and layout order, for the usage popup.
#[derive(Clone)]
pub(crate) struct TabProcesses {
    pub title: SharedString,
    pub active: bool,
    pub panes: Vec<PaneProcesses>,
}

#[derive(Clone)]
pub(crate) struct PaneProcesses {
    pub title: SharedString,
    /// `None` when the pane has no running process.
    pub root: Option<ProcessRoot>,
}

#[derive(Clone)]
struct UsagePopup {
    details: Vec<(&'static str, SharedString)>,
    tabs: Vec<TabRow>,
    gpu: bool,
}

#[derive(Clone)]
struct TabRow {
    title: SharedString,
    active: bool,
    usage: Option<TreeUsage>,
    /// Listed only when the tab is split.
    panes: Vec<PaneRow>,
}

#[derive(Clone)]
struct PaneRow {
    title: SharedString,
    usage: Option<TreeUsage>,
}

impl UsagePopup {
    fn render(&self) -> AnyElement {
        let mut rows = Vec::new();
        for (index, tab) in self.tabs.iter().enumerate() {
            let color = if tab.active {
                theme::FG
            } else {
                theme::FG_MUTED
            };
            rows.push(self.row(
                ("usage-tab", index),
                &tab.title,
                tab.usage.as_ref(),
                0.0,
                color,
            ));
            for (pane, row) in tab.panes.iter().enumerate() {
                let id = ("usage-pane", index * MAX_PANES_PER_TAB + pane);
                rows.push(self.row(
                    id,
                    &row.title,
                    row.usage.as_ref(),
                    POPUP_INDENT_PX,
                    theme::GREY,
                ));
            }
        }
        div()
            .w(px(POPUP_WIDTH_PX))
            .flex()
            .flex_col()
            .text_size(px(FONT_SIZE_PX))
            .text_color(gpui_kit::rgb(theme::FG_MUTED))
            .child(
                div()
                    .px(px(POPUP_PADDING_X_PX))
                    .pb(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .children(self.details.iter().map(|(label, detail)| {
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .w(px(34.0))
                                    .flex_shrink_0()
                                    .text_color(gpui_kit::rgb(theme::GREY))
                                    .child(*label),
                            )
                            .child(div().min_w_0().child(detail.clone()))
                    })),
            )
            .child(
                self.cells_row(
                    div()
                        .h(px(POPUP_ROW_HEIGHT_PX))
                        .border_t_1()
                        .border_color(gpui_kit::rgb(theme::CHROME_BORDER))
                        .text_size(px(10.0))
                        .text_color(gpui_kit::rgb(theme::GREY)),
                    div().child("Tab / pane").into_any_element(),
                    None,
                    ["CPU", "MEM", "GPU"].map(String::from),
                ),
            )
            .child(
                div()
                    .id("system-usage-panes")
                    .max_h(px(POPUP_LIST_MAX_HEIGHT_PX))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .child(
                div()
                    .px(px(POPUP_PADDING_X_PX))
                    .pt(px(6.0))
                    .border_t_1()
                    .border_color(gpui_kit::rgb(theme::CHROME_BORDER))
                    .text_size(px(10.0))
                    .text_color(gpui_kit::rgb(theme::GREY))
                    .child(
                        "Each pane counts its shell and every process started under it. \
                         CPU is a share of all processors; memory is private working set.",
                    ),
            )
            .into_any_element()
    }

    fn row(
        &self,
        id: (&'static str, usize),
        title: &SharedString,
        usage: Option<&TreeUsage>,
        indent: f32,
        color: u32,
    ) -> AnyElement {
        let note = usage.map(|usage| match usage.processes {
            1 => "1 process".to_string(),
            count => format!("{count} processes"),
        });
        self.cells_row(
            div()
                .id(id)
                .h(px(POPUP_ROW_HEIGHT_PX))
                .hover(|row| row.bg(gpui_kit::rgb(theme::ROW_ACTIVE))),
            div()
                .pl(px(indent))
                .min_w_0()
                .truncate()
                .text_color(gpui_kit::rgb(color))
                .child(title.clone())
                .into_any_element(),
            note,
            usage_cells(usage),
        )
    }

    /// A row with a title, an optional muted note, and the value columns.
    fn cells_row<E: Styled + ParentElement + IntoElement>(
        &self,
        row: E,
        title: AnyElement,
        note: Option<String>,
        cells: [String; 3],
    ) -> AnyElement {
        let columns = if self.gpu { 3 } else { 2 };
        row.flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(8.0))
            .px(px(POPUP_PADDING_X_PX))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(title)
                    .children(note.map(|note| {
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.0))
                            .text_color(gpui_kit::rgb(theme::GREY_DIM))
                            .child(note)
                    })),
            )
            .children(cells.into_iter().take(columns).map(|cell| {
                div()
                    .w(px(POPUP_COLUMN_WIDTH_PX))
                    .flex_shrink_0()
                    .flex()
                    .justify_end()
                    .font_family(crate::widget::DEFAULT_TERMINAL_FONT_FAMILY)
                    .child(cell)
            }))
            .into_any_element()
    }
}

/// CPU, memory and GPU for a tree: a dash when it is not running, and an
/// ellipsis for shares until a second sample arrives.
fn usage_cells(usage: Option<&TreeUsage>) -> [String; 3] {
    let Some(usage) = usage else {
        return ["\u{2013}", "\u{2013}", "\u{2013}"].map(String::from);
    };
    let pending = || "\u{2026}".to_string();
    let share = |share: f32| format!("{}%", (share.clamp(0.0, 1.0) * 100.0).round() as u32);
    let gpu = match (usage.cpu, usage.engines.iter().copied().reduce(f32::max)) {
        (None, _) => pending(),
        (Some(_), Some(busiest)) => share(busiest),
        (Some(_), None) => "\u{2013}".to_string(),
    };
    [
        usage.cpu.map_or_else(pending, share),
        format_size(usage.memory),
        gpu,
    ]
}

/// A size in MiB below 1000 MiB, else in GiB with one decimal.
fn format_size(bytes: u64) -> String {
    let mib = bytes as f64 / (1u64 << 20) as f64;
    if mib < 999.5 {
        format!("{mib:.0}M")
    } else {
        format!("{:.1}G", bytes as f64 / GIB)
    }
}

fn push_history(history: &mut VecDeque<f32>, value: Option<f32>) {
    let Some(value) = value else {
        history.clear();
        return;
    };
    if history.len() == HISTORY_LEN {
        history.pop_front();
    }
    history.push_back(value);
}

enum Graph {
    Sparkline(Vec<f32>, u32),
    Bar(f32, u32),
    None,
}

struct Meter {
    id: &'static str,
    label: &'static str,
    graph: Graph,
    value: String,
    /// Muted text after the value, such as the capacity.
    suffix: Option<String>,
    /// Exact figures, shown in the per-pane popup.
    detail: String,
}

impl Meter {
    fn width(&self, graph: bool) -> f32 {
        let graph = if graph && !matches!(self.graph, Graph::None) {
            GRAPH_WIDTH_PX + METER_INNER_GAP_PX
        } else {
            0.0
        };
        let value_chars = self.value.len() + self.suffix.as_ref().map_or(0, String::len);
        self.label.len() as f32 * LABEL_CHAR_PX
            + METER_INNER_GAP_PX
            + graph
            + value_chars as f32 * VALUE_CHAR_PX
    }

    fn render(self, graph: bool) -> AnyElement {
        let graph = match self.graph {
            Graph::Sparkline(values, color) if graph => Some(sparkline(values, color)),
            Graph::Bar(fraction, color) if graph => Some(bar(fraction, color)),
            _ => None,
        };
        div()
            .id(self.id)
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(METER_INNER_GAP_PX))
            .child(self.label)
            .children(graph)
            .child(
                div()
                    .flex()
                    .font_family(crate::widget::DEFAULT_TERMINAL_FONT_FAMILY)
                    .text_color(gpui_kit::rgb(theme::FG_MUTED))
                    .child(self.value)
                    .children(
                        self.suffix.map(|suffix| {
                            div().text_color(gpui_kit::rgb(theme::GREY)).child(suffix)
                        }),
                    ),
            )
            .into_any_element()
    }
}

/// A line over the recent samples, newest at the right edge.
fn sparkline(values: Vec<f32>, color: u32) -> AnyElement {
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            if values.len() < 2 {
                return;
            }
            let step = bounds.size.width / (HISTORY_LEN - 1) as f32;
            let inset = px(SPARKLINE_STROKE_PX);
            let range = bounds.size.height - inset * 2.0;
            let mut path = gpui_kit::PathBuilder::stroke(px(SPARKLINE_STROKE_PX));
            for (index, value) in values.iter().enumerate() {
                let x = bounds.right() - step * (values.len() - 1 - index) as f32;
                let y = bounds.bottom() - inset - range * value.clamp(0.0, 1.0);
                if index == 0 {
                    path.move_to(point(x, y));
                } else {
                    path.line_to(point(x, y));
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, gpui_kit::rgb(color));
            }
        },
    )
    .w(px(GRAPH_WIDTH_PX))
    .h(px(SPARKLINE_HEIGHT_PX))
    .flex_shrink_0()
    .into_any_element()
}

fn bar(fraction: f32, color: u32) -> AnyElement {
    div()
        .w(px(GRAPH_WIDTH_PX))
        .h(px(BAR_HEIGHT_PX))
        .flex_shrink_0()
        .rounded(px(BAR_HEIGHT_PX / 2.0))
        .bg(gpui_kit::rgb(theme::KEYCAP))
        .child(
            div()
                .h_full()
                .w(px(GRAPH_WIDTH_PX * fraction.clamp(0.0, 1.0)))
                .rounded(px(BAR_HEIGHT_PX / 2.0))
                .bg(gpui_kit::rgb(color)),
        )
        .into_any_element()
}

/// How many meters fit, and whether with graphs. `widths` holds each meter's
/// width with and without its graph.
fn fit(widths: &[(f32, f32)], available: f32) -> Option<(usize, bool)> {
    (1..=widths.len()).rev().find_map(|count| {
        [true, false].into_iter().find_map(|graphs| {
            let meters: f32 = widths[..count]
                .iter()
                .map(|&(with, without)| if graphs { with } else { without })
                .sum();
            let total = meters + METER_GAP_PX * (count - 1) as f32 + PADDING_X_PX * 2.0;
            (total <= available).then_some((count, graphs))
        })
    })
}

/// A share as a whole percentage, padded on the right to the width of "100%".
fn percent(share: f32) -> String {
    let percent = format!("{}%", (share.clamp(0.0, 1.0) * 100.0).round() as u32);
    format!("{percent:<PERCENT_CHARS$}")
}

/// An amount in GiB: one decimal below 100.
fn format_gib(bytes: u64) -> String {
    let gib = bytes as f64 / GIB;
    if gib < 99.95 {
        format!("{gib:.1}")
    } else {
        format!("{gib:.0}")
    }
}

/// A capacity in GiB: whole when it is close to whole or at least 10.
fn format_capacity(bytes: u64) -> String {
    let gib = bytes as f64 / GIB;
    if gib >= 10.0 || (gib - gib.round()).abs() < 0.05 {
        format!("{:.0}", gib.round())
    } else {
        format!("{gib:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_amounts_compactly() {
        assert_eq!(percent(0.0), "0%  ");
        assert_eq!(percent(0.064), "6%  ");
        assert_eq!(percent(1.2), "100%");
        assert_eq!(format_gib(1288490188), "1.2");
        assert_eq!(format_gib(200 << 30), "200");
        assert_eq!(format_capacity(32 << 30), "32");
        assert_eq!(format_capacity(34_255_360_000), "32");
        assert_eq!(format_capacity(15_917 << 20), "16");
        assert_eq!(format_capacity(3 << 29), "1.5");
        assert_eq!(format_capacity(8 << 30), "8");
    }

    #[test]
    fn narrow_title_bars_drop_graphs_before_meters() {
        let widths = [(100.0, 60.0), (120.0, 80.0), (100.0, 60.0)];
        let padding = PADDING_X_PX * 2.0;
        let all = 320.0 + METER_GAP_PX * 2.0 + padding;
        assert_eq!(fit(&widths, all), Some((3, true)));
        assert_eq!(fit(&widths, all - 1.0), Some((3, false)));
        let compact = 200.0 + METER_GAP_PX * 2.0 + padding;
        assert_eq!(fit(&widths, compact - 1.0), Some((2, false)));
        assert_eq!(fit(&widths, 10.0), None);
    }

    #[test]
    fn history_keeps_recent_samples_and_restarts_after_a_gap() {
        let mut history = VecDeque::new();
        for value in 0..HISTORY_LEN + 3 {
            push_history(&mut history, Some(value as f32));
        }
        assert_eq!(history.len(), HISTORY_LEN);
        assert_eq!(history.front(), Some(&3.0));
        push_history(&mut history, None);
        assert!(history.is_empty());
    }
}
