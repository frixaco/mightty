use crate::ghostty::{
    render::CellWidth,
    style::{RgbColor, Underline},
};
use gpui::{
    Bounds, ContentMask, Context, FontFallbacks, FontFeatures, FontStyle, FontWeight, IntoElement,
    KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Render, ScrollWheelEvent, StrikethroughStyle, Styled, TextRun, TextStyle, UnderlineStyle,
    WhiteSpace, Window, canvas, div, fill, point, prelude::*, px, size,
};
#[cfg(test)]
use std::cell::RefCell;
use std::sync::Arc;

use super::{TERMINAL_KEY_CONTEXT, TerminalWidget, rgb_to_rgba, scrollbar_layout};

#[cfg(test)]
thread_local! {
    static RENDER_STAGE_TRACE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn reset_render_stage_trace() {
    RENDER_STAGE_TRACE.with(|trace| trace.borrow_mut().clear());
}

#[cfg(test)]
fn record_render_stage(stage: impl Into<String>) {
    RENDER_STAGE_TRACE.with(|trace| trace.borrow_mut().push(stage.into()));
}

#[cfg(test)]
fn render_stage_trace() -> Vec<String> {
    RENDER_STAGE_TRACE.with(|trace| trace.borrow().clone())
}

pub(super) trait CellWidthExt {
    fn column_advance(self) -> u16;
}

impl CellWidthExt for CellWidth {
    fn column_advance(self) -> u16 {
        match self {
            Self::Narrow => 1,
            Self::Wide => 2,
            Self::SpacerTail | Self::SpacerHead => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RowTextStyle {
    fg: RgbColor,
    bg: Option<RgbColor>,
    default_bg: RgbColor,
    bold: bool,
    italic: bool,
    underline: Underline,
    strikethrough: bool,
}

struct RowSegment {
    start_col: u16,
    columns: u16,
    text: String,
    style: RowTextStyle,
}

impl RowSegment {
    fn new(start_col: u16, columns: u16, text: String, style: RowTextStyle) -> Self {
        Self {
            start_col,
            columns,
            text,
            style,
        }
    }
}

impl Render for TerminalWidget {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prepare_terminal(window, cx);
        self.paint_terminal(window, cx, false)
    }
}

impl TerminalWidget {
    /// Prepare the same painter on established geometry without live lifecycle effects.
    pub fn offscreen_element(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.publish_terminal(cx);
        self.paint_terminal(window, cx, true).into_any_element()
    }

    fn paint_terminal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        offscreen: bool,
    ) -> gpui::Div {
        #[cfg(test)]
        reset_render_stage_trace();

        if !offscreen && self.blur_subscription.is_none() {
            self.blur_subscription =
                Some(cx.on_blur(&self.focus_handle, window, |this, _window, cx| {
                    this.preedit.clear();
                    this.composing = false;
                    this.preedit_selection = 0..0;
                    this.pending_text_key = None;
                    this.pressed_keys.clear();
                    cx.notify();
                }));
        }

        let Some(frame) = self.committed.clone() else {
            return div().size_full().bg(self.theme.background);
        };
        let painted = if offscreen {
            &mut self.offscreen_painted
        } else {
            &mut self.painted
        };
        *painted = Some(super::presentation::PaintedTerminal {
            frame: frame.clone(),
            focused: self.focus_handle.is_focused(window),
            cursor_phase: self.cursor_blink_phase,
            preedit: self.preedit.clone(),
            composing: self.composing,
        });
        let colors = &frame.colors;
        let cell_size = frame.cell_size;
        let layout_size = self
            .layout_bounds
            .map_or(window.viewport_size(), |b| b.size);
        let selection_color = frame.selection_color;
        let search_highlights = frame.highlights.clone();
        let graphics = frame.graphics.clone();
        #[cfg(test)]
        record_render_stage(format!(
            "below-background graphics z={:?}",
            graphics
                .below_background
                .iter()
                .map(|placement| placement.z())
                .collect::<Vec<_>>()
        ));
        let mut elements: Vec<gpui::AnyElement> = graphics
            .below_background
            .into_iter()
            .map(|placement| placement.into_element())
            .collect();
        let mut backgrounds = Vec::new();
        let mut text_segments = Vec::new();
        let mut base_text_style = window.text_style();
        base_text_style.font_family = frame.font_family.clone().into();
        base_text_style.font_features = terminal_font_features();
        base_text_style.font_fallbacks = Some(terminal_font_fallbacks(&frame.font_family));
        base_text_style.font_size = px(frame.font_size_px).into();
        base_text_style.line_height = cell_size.1.into();
        base_text_style.white_space = WhiteSpace::Nowrap;

        for (row_index, row) in frame.rows.iter().enumerate() {
            let row_idx = row_index as u16;
            let row_selection = row.selection;
            let mut row_segments = Vec::new();
            let mut pending_segment = None;
            for (column, cell) in row.cells.iter().enumerate() {
                let width = cell.width;
                let advance = width.column_advance();
                let start_col = column as u16;
                let text = cell.text.as_str();
                if matches!(width, CellWidth::SpacerTail | CellWidth::SpacerHead) {
                    continue;
                }

                let fg = cell.foreground.unwrap_or(colors.foreground);
                let bg = cell.background;
                let style = cell.style;
                let (fg_color, bg_color, has_bg) = if style.inverse {
                    (fg, bg.unwrap_or(colors.background), true)
                } else {
                    (fg, bg.unwrap_or(colors.background), bg.is_some())
                };

                let segment = if text.is_empty() {
                    if advance == 2 { "  " } else { " " }
                } else {
                    text
                };
                let background = selected_background(
                    row_selection,
                    start_col,
                    advance.max(1),
                    (has_bg || style.inverse).then_some(bg_color),
                    selection_color,
                );
                push_row_segment(
                    &mut row_segments,
                    &mut pending_segment,
                    start_col,
                    advance.max(1),
                    RowTextStyle {
                        fg: fg_color,
                        bg: background,
                        default_bg: colors.background,
                        bold: style.bold,
                        italic: style.italic,
                        underline: style.underline,
                        strikethrough: style.strikethrough,
                    },
                    segment,
                );
            }

            if let Some(segment) = pending_segment.take() {
                row_segments.push(segment);
            }

            for segment in row_segments {
                let (x, y) = cell_position(row_idx, segment.start_col, cell_size);
                let segment_width = cell_size.0 * segment.columns as f32;
                let bounds = Bounds::new(point(x, y), size(segment_width, cell_size.1));
                let (_, segment_bg, _) = resolved_render_style(segment.style);
                if let Some(background) = segment_bg {
                    backgrounds.push((bounds, rgb_to_rgba(background)));
                }
                text_segments.push((bounds, segment));
            }
        }

        #[cfg(test)]
        record_render_stage(format!("cell backgrounds count={}", backgrounds.len()));
        elements.push(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    for (cell_bounds, color) in backgrounds {
                        let cell_bounds =
                            pixel_aligned_bounds(cell_bounds, bounds.origin, window.scale_factor());
                        window.paint_quad(fill(cell_bounds, color));
                    }
                },
            )
            .absolute()
            .size_full()
            .into_any_element(),
        );
        #[cfg(test)]
        record_render_stage(format!(
            "search highlights count={}",
            search_highlights.len()
        ));
        elements.extend(search_highlights.into_iter().map(|highlight| {
            let color = if highlight.active {
                gpui::rgba(0xffa500cc)
            } else {
                gpui::rgba(0xffd54f66)
            };
            div()
                .absolute()
                .left(cell_size.0 * f32::from(highlight.start_column))
                .top(cell_size.1 * f32::from(highlight.row))
                .w(cell_size.0 * f32::from(highlight.end_column - highlight.start_column + 1))
                .h(cell_size.1)
                .bg(color)
                .into_any_element()
        }));
        #[cfg(test)]
        record_render_stage(format!(
            "below-text graphics z={:?}",
            graphics
                .below_text
                .iter()
                .map(|placement| placement.z())
                .collect::<Vec<_>>()
        ));
        elements.extend(
            graphics
                .below_text
                .into_iter()
                .map(|placement| placement.into_element()),
        );
        #[cfg(test)]
        record_render_stage(format!("cell text count={}", text_segments.len()));
        let font_size = px(frame.font_size_px);
        elements.push(
            canvas(
                move |_, window, _| {
                    text_segments
                        .into_iter()
                        .map(|(bounds, segment)| {
                            let run = text_run_for_style(
                                &base_text_style,
                                segment.style,
                                segment.text.len(),
                            );
                            let line = window.text_system().shape_line(
                                segment.text.into(),
                                font_size,
                                &[run],
                                None,
                            );
                            (bounds, line)
                        })
                        .collect::<Vec<_>>()
                },
                move |bounds, lines, window, cx| {
                    for (segment_bounds, line) in lines {
                        let segment_bounds = pixel_aligned_bounds(
                            segment_bounds,
                            bounds.origin,
                            window.scale_factor(),
                        );
                        window.with_content_mask(
                            Some(ContentMask {
                                bounds: segment_bounds,
                            }),
                            |window| {
                                if let Err(error) =
                                    line.paint(segment_bounds.origin, cell_size.1, window, cx)
                                {
                                    crate::diagnostics::record(
                                        "render",
                                        "text_paint_failed",
                                        &error.to_string(),
                                        serde_json::json!({}),
                                    );
                                }
                            },
                        );
                    }
                },
            )
            .absolute()
            .size_full()
            .into_any_element(),
        );

        if !self.preedit.is_empty()
            && let Some((column, row, _)) = frame.cursor_footprint()
        {
            let (x, y) = cell_position(row, column, cell_size);
            elements.push(
                div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .h(cell_size.1)
                    .font_family(frame.font_family.clone())
                    .text_size(px(frame.font_size_px))
                    .line_height(cell_size.1)
                    .whitespace_nowrap()
                    .bg(rgb_to_rgba(colors.background))
                    .text_color(rgb_to_rgba(colors.foreground))
                    .border_b_1()
                    .border_color(rgb_to_rgba(colors.foreground))
                    .child(self.preedit.clone())
                    .into_any_element(),
            );
        }

        let cursor_visible = self.focus_handle.is_focused(window)
            && frame.cursor.visible
            && (self.cursor_blink_phase || !frame.cursor.blinking)
            && !self.composing;
        if cursor_visible && let Some((column, row, columns)) = frame.cursor_footprint() {
            use crate::ghostty::render::CursorShape;

            let cell = frame
                .rows
                .get(usize::from(row))
                .and_then(|row| row.cells.get(usize::from(column)));
            let width = cell_size.0 * f32::from(columns);
            let (x, y) = cell_position(row, column, cell_size);
            let color = rgb_to_rgba(colors.cursor.unwrap_or(colors.foreground));
            let cursor_div = match frame.cursor.shape {
                CursorShape::Block => div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(width)
                    .h(cell_size.1)
                    .bg(color),
                CursorShape::HollowBlock => div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(width)
                    .h(cell_size.1)
                    .border_1()
                    .border_color(color),
                CursorShape::Bar => div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(px(2.0))
                    .h(cell_size.1)
                    .bg(color),
                CursorShape::Underline => div()
                    .absolute()
                    .left(x)
                    .top(y + cell_size.1 - px(2.0))
                    .w(width)
                    .h(px(2.0))
                    .bg(color),
            };
            elements.push(cursor_div.into_any_element());
            if frame.cursor.shape == CursorShape::Block
                && let Some(cell) = cell
            {
                let mut cursor_style = window.text_style();
                cursor_style.font_family = frame.font_family.clone().into();
                cursor_style.font_features = terminal_font_features();
                cursor_style.font_fallbacks = Some(terminal_font_fallbacks(&frame.font_family));
                let text_style = RowTextStyle {
                    fg: cursor_text_color(
                        colors.cursor.unwrap_or(colors.foreground),
                        colors.background,
                    ),
                    bg: None,
                    default_bg: colors.background,
                    bold: cell.style.bold,
                    italic: cell.style.italic,
                    underline: cell.style.underline,
                    strikethrough: cell.style.strikethrough,
                };
                let text = cell.text.clone();
                elements.push(
                    canvas(
                        move |_, window, _| {
                            let run = text_run_for_style(&cursor_style, text_style, text.len());
                            window
                                .text_system()
                                .shape_line(text.into(), font_size, &[run], None)
                        },
                        move |bounds, line, window, cx| {
                            let bounds = pixel_aligned_bounds(
                                Bounds::new(point(x, y), size(width, cell_size.1)),
                                bounds.origin,
                                window.scale_factor(),
                            );
                            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                                let _ = line.paint(bounds.origin, cell_size.1, window, cx);
                            });
                        },
                    )
                    .absolute()
                    .size_full()
                    .into_any_element(),
                );
            }
        }
        #[cfg(test)]
        record_render_stage(format!(
            "cursor count={}",
            usize::from(cursor_visible && frame.cursor.position.is_some())
        ));
        #[cfg(test)]
        record_render_stage(format!(
            "above-text graphics z={:?}",
            graphics
                .above_text
                .iter()
                .map(|placement| placement.z())
                .collect::<Vec<_>>()
        ));
        elements.extend(
            graphics
                .above_text
                .into_iter()
                .map(|placement| placement.into_element()),
        );

        let entity = cx.entity();
        let input_entity = entity.clone();
        let input_focus = self.focus_handle.clone();
        let track_height: f32 = layout_size.height.into();
        let scrollbar = Some(frame.scrollbar)
            .and_then(|scrollbar| scrollbar_layout(scrollbar, track_height))
            .map(|layout| {
                div()
                    .absolute()
                    .right(px(0.0))
                    .top(px(0.0))
                    .h_full()
                    .w(px(10.0))
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.handle_scrollbar_down(event, window, cx)
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                        this.handle_scrollbar_move(event, cx)
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseUpEvent, window, cx| {
                            this.handle_scrollbar_up(event, window, cx)
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseUpEvent, window, cx| {
                            this.handle_scrollbar_up(event, window, cx)
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(px(2.0))
                            .top(px(layout.top))
                            .w(px(6.0))
                            .h(px(layout.height))
                            .rounded(px(3.0))
                            .bg(gpui::rgba(0xffffff55)),
                    )
            });
        let paste_confirmation = self.pending_paste.as_ref().map(|text| {
            let line_count = text.lines().count().max(1);
            let detail = format!("{line_count} lines, {} bytes", text.len());
            div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x000000aa))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _event: &MouseDownEvent, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .w(px(420.0))
                        .p(px(20.0))
                        .rounded(px(8.0))
                        .bg(gpui::rgb(0x202020))
                        .text_color(gpui::white())
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            div()
                                .text_size(px(16.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Confirm paste"),
                        )
                        .child(div().text_size(px(13.0)).child(
                            "This paste can run commands. Review the source before you continue.",
                        ))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(gpui::rgb(0xa0a0a0))
                                .child(detail),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .id("cancel-unsafe-paste")
                                        .px(px(14.0))
                                        .py(px(8.0))
                                        .rounded(px(4.0))
                                        .bg(gpui::rgb(0x383838))
                                        .hover(|style| style.bg(gpui::rgb(0x484848)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(
                                                |this, _event: &MouseDownEvent, window, cx| {
                                                    this.cancel_pending_paste(cx);
                                                    window.prevent_default();
                                                    cx.stop_propagation();
                                                },
                                            ),
                                        )
                                        .child("Cancel"),
                                )
                                .child(
                                    div()
                                        .id("confirm-unsafe-paste")
                                        .px(px(14.0))
                                        .py(px(8.0))
                                        .rounded(px(4.0))
                                        .bg(gpui::rgb(0x0e639c))
                                        .hover(|style| style.bg(gpui::rgb(0x1177bb)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(
                                                |this, _event: &MouseDownEvent, window, cx| {
                                                    this.confirm_pending_paste(cx);
                                                    window.prevent_default();
                                                    cx.stop_propagation();
                                                },
                                            ),
                                        )
                                        .child("Paste"),
                                ),
                        ),
                )
        });
        // Use a disposable input entity for the readonly overlay so painting in a
        // scratch window cannot change the live input's bounds or handler.
        let search_input = if offscreen {
            self.search.as_ref().map(|search| {
                cx.new(|cx| {
                    let mut input = gpui_component::input::InputState::new(window, cx);
                    input.set_value(search.query.clone(), window, cx);
                    input
                })
            })
        } else {
            self.search_input.clone()
        };
        let search_overlay = self.search.as_ref().map(|search| {
            let diagnostic = search.diagnostic.as_ref().map(|diagnostic| {
                div()
                    .text_size(px(11.0))
                    .text_color(gpui::rgb(0xff8a80))
                    .child(diagnostic.clone())
            });
            div()
                .absolute()
                .top(px(10.0))
                .right(px(18.0))
                .w(px(420.0))
                .p(px(10.0))
                .rounded(px(6.0))
                .bg(gpui::rgba(0x20242aee))
                .border_1()
                .border_color(gpui::rgb(0x4b5563))
                .text_color(gpui::white())
                .occlude()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .children(search_input.as_ref().map(gpui_component::input::Input::new))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(gpui::rgb(0xaeb8c4))
                                .child(search.result_label()),
                        ),
                )
                .children(diagnostic)
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(gpui::rgb(0x8c98a8))
                        .child("Enter: next · Shift+Enter: previous · Esc: close"),
                )
        });

        if offscreen {
            return div()
                .size_full()
                .bg(rgb_to_rgba(colors.background))
                .relative()
                .overflow_hidden()
                .children(elements)
                .children(scrollbar)
                .children(paste_confirmation)
                .children(search_overlay);
        }
        div()
            .size_full()
            .bg(rgb_to_rgba(colors.background))
            .relative()
            .overflow_hidden()
            .key_context(TERMINAL_KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .capture_action(cx.listener(
                |this, event: &gpui_component::input::Enter, _window, cx| {
                    if this.search.is_some() {
                        this.navigate_search(
                            if event.secondary {
                                crate::ghostty::SearchDirection::Previous
                            } else {
                                crate::ghostty::SearchDirection::Next
                            },
                            cx,
                        );
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                },
            ))
            .on_action(cx.listener(Self::handle_tab_action))
            .on_action(cx.listener(Self::handle_tab_prev_action))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key_down(event, window, cx)
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, window, cx| {
                this.handle_key_up(event, window, cx)
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.handle_mouse_down(event, window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.handle_mouse_down(event, window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.handle_mouse_down(event, window, cx)
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.handle_mouse_move(event, cx)
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.handle_mouse_up(event, window, cx)
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.handle_mouse_up(event, window, cx)
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.handle_mouse_up(event, window, cx)
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.handle_mouse_up(event, window, cx)
                }),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.handle_mouse_up(event, window, cx)
                }),
            )
            .on_mouse_up_out(
                MouseButton::Middle,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.handle_mouse_up(event, window, cx)
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                this.handle_scroll_wheel(event, window, cx)
            }))
            .children(elements)
            .children(scrollbar)
            .child(
                canvas(
                    move |bounds, _window, cx| {
                        entity.update(cx, |this, cx| {
                            this.update_layout_bounds(bounds, cx);
                        });
                    },
                    move |bounds, _, window, cx| {
                        window.handle_input(
                            &input_focus,
                            gpui::ElementInputHandler::new(bounds, input_entity.clone()),
                            cx,
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(paste_confirmation)
            .children(search_overlay)
    }
}

fn selected_background(
    selection: Option<crate::ghostty::render::RowSelection>,
    start_col: u16,
    columns: u16,
    background: Option<RgbColor>,
    selection_color: RgbColor,
) -> Option<RgbColor> {
    let Some(selection) = selection else {
        return background;
    };
    let end_col = start_col.saturating_add(columns.saturating_sub(1));
    if start_col <= selection.end && end_col >= selection.start {
        Some(selection_color)
    } else {
        background
    }
}

pub(super) fn terminal_font_features() -> FontFeatures {
    FontFeatures(Arc::new(vec![
        ("calt".to_string(), 0),
        ("liga".to_string(), 0),
        ("kern".to_string(), 0),
    ]))
}

pub(super) fn terminal_font_fallbacks(primary: &str) -> FontFallbacks {
    FontFallbacks::from_fonts(vec![
        primary.to_string(),
        "Consolas".to_string(),
        "Cascadia Mono".to_string(),
        "DejaVu Sans Mono".to_string(),
        "Noto Sans Mono".to_string(),
        "JetBrains Mono".to_string(),
        "Fira Mono".to_string(),
        "Sarasa Mono SC".to_string(),
        "Sarasa Term SC".to_string(),
        "Sarasa Mono J".to_string(),
        "Noto Sans Mono CJK SC".to_string(),
        "Noto Sans Mono CJK JP".to_string(),
        "Source Han Mono SC".to_string(),
        "WenQuanYi Zen Hei Mono".to_string(),
        "Apple Color Emoji".to_string(),
        "Noto Color Emoji".to_string(),
        "Segoe UI Emoji".to_string(),
    ])
}

fn mix_rgb(a: RgbColor, b: RgbColor, ratio: f32) -> RgbColor {
    let t = ratio.clamp(0.0, 1.0);
    let blend = |lhs: u8, rhs: u8| -> u8 {
        ((lhs as f32 * (1.0 - t)) + (rhs as f32 * t))
            .round()
            .clamp(0.0, 255.0) as u8
    };

    RgbColor {
        r: blend(a.r, b.r),
        g: blend(a.g, b.g),
        b: blend(a.b, b.b),
    }
}

fn rgb_to_hsv(rgb: RgbColor) -> (f32, f32, f32) {
    let r = rgb.r as f32 / 255.0;
    let g = rgb.g as f32 / 255.0;
    let b = rgb.b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };

    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    (hue, saturation, max)
}

fn bold_display_palette_color(rgb: RgbColor, base_bg: RgbColor) -> RgbColor {
    let (hue, saturation, value) = rgb_to_hsv(rgb);

    if saturation < 0.16 || value < 0.2 {
        return if relative_luminance(base_bg) < 0.35 {
            RgbColor {
                r: 230,
                g: 237,
                b: 243,
            }
        } else {
            RgbColor {
                r: 30,
                g: 41,
                b: 59,
            }
        };
    }

    match hue {
        h if !(15.0..345.0).contains(&h) => RgbColor {
            r: 255,
            g: 123,
            b: 114,
        },
        h if h < 45.0 => RgbColor {
            r: 255,
            g: 184,
            b: 108,
        },
        h if h < 70.0 => RgbColor {
            r: 229,
            g: 192,
            b: 123,
        },
        h if h < 150.0 => RgbColor {
            r: 152,
            g: 195,
            b: 121,
        },
        h if h < 210.0 => RgbColor {
            r: 86,
            g: 212,
            b: 221,
        },
        h if h < 270.0 => RgbColor {
            r: 97,
            g: 175,
            b: 239,
        },
        _ => RgbColor {
            r: 198,
            g: 120,
            b: 221,
        },
    }
}

fn relative_luminance(rgb: RgbColor) -> f32 {
    fn channel(value: u8) -> f32 {
        let normalized = value as f32 / 255.0;
        if normalized <= 0.03928 {
            normalized / 12.92
        } else {
            ((normalized + 0.055) / 1.055).powf(2.4)
        }
    }

    0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b)
}

fn contrast_ratio(a: RgbColor, b: RgbColor) -> f32 {
    let a_lum = relative_luminance(a);
    let b_lum = relative_luminance(b);
    let lighter = a_lum.max(b_lum);
    let darker = a_lum.min(b_lum);
    (lighter + 0.05) / (darker + 0.05)
}

fn emphasized_bold_colors(style: RowTextStyle) -> (RgbColor, Option<RgbColor>) {
    let base_bg = style.bg.unwrap_or(style.default_bg);
    let mut fg = bold_display_palette_color(style.fg, base_bg);
    let target = if relative_luminance(base_bg) < 0.35 {
        RgbColor {
            r: 255,
            g: 255,
            b: 255,
        }
    } else {
        RgbColor { r: 0, g: 0, b: 0 }
    };

    if contrast_ratio(fg, base_bg) < 7.0 {
        for ratio in [0.55_f32, 0.7, 0.82, 0.9] {
            let candidate = mix_rgb(fg, target, ratio);
            fg = candidate;
            if contrast_ratio(fg, base_bg) >= 7.0 {
                break;
            }
        }
    }

    (fg, style.bg)
}

fn resolved_render_style(style: RowTextStyle) -> (RgbColor, Option<RgbColor>, FontWeight) {
    if style.bold {
        let (fg, bg) = emphasized_bold_colors(style);
        (fg, bg, FontWeight::BOLD)
    } else {
        (style.fg, style.bg, FontWeight::NORMAL)
    }
}

fn text_run_for_style(base_style: &TextStyle, style: RowTextStyle, len: usize) -> TextRun {
    let mut run_style = base_style.clone();
    let (fg, _bg, font_weight) = resolved_render_style(style);
    run_style.color = rgb_to_rgba(fg).into();
    run_style.background_color = None;
    run_style.font_weight = font_weight;
    run_style.font_style = if style.italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    run_style.underline = match style.underline {
        Underline::None => None,
        Underline::Curly => Some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(rgb_to_rgba(fg).into()),
            wavy: true,
        }),
        _ => Some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(rgb_to_rgba(fg).into()),
            wavy: false,
        }),
    };
    run_style.strikethrough = style.strikethrough.then_some(StrikethroughStyle {
        thickness: px(1.0),
        color: Some(rgb_to_rgba(fg).into()),
    });
    run_style.to_run(len)
}

fn segment_needs_own_layout(segment: &str, columns: u16) -> bool {
    columns != 1 || !segment.is_ascii()
}

fn push_row_segment(
    segments: &mut Vec<RowSegment>,
    pending: &mut Option<RowSegment>,
    start_col: u16,
    columns: u16,
    style: RowTextStyle,
    text: &str,
) {
    if text.is_empty() {
        return;
    }

    let isolate = segment_needs_own_layout(text, columns);
    if isolate {
        if let Some(segment) = pending.take() {
            segments.push(segment);
        }
        segments.push(RowSegment::new(start_col, columns, text.to_owned(), style));
        return;
    }

    if let Some(segment) = pending.as_mut()
        && segment.style == style
        && segment.start_col + segment.columns == start_col
    {
        segment.columns += columns;
        segment.text.push_str(text);
        return;
    }

    if let Some(segment) = pending.take() {
        segments.push(segment);
    }
    *pending = Some(RowSegment::new(start_col, columns, text.to_owned(), style));
}

fn cell_position(row: u16, col: u16, cell_size: (Pixels, Pixels)) -> (Pixels, Pixels) {
    (cell_size.0 * col as f32, cell_size.1 * row as f32)
}

fn cursor_text_color(cursor: RgbColor, background: RgbColor) -> RgbColor {
    let brightness = |color: RgbColor| {
        (u32::from(color.r) * 299 + u32::from(color.g) * 587 + u32::from(color.b) * 114) / 1000
    };
    if brightness(cursor).abs_diff(brightness(background)) >= 96 {
        background
    } else if brightness(cursor) >= 128 {
        RgbColor { r: 0, g: 0, b: 0 }
    } else {
        RgbColor {
            r: 255,
            g: 255,
            b: 255,
        }
    }
}

// Match GPUI's layout rounding for the positioned spans replaced by the canvases.
fn pixel_aligned_bounds(
    bounds: Bounds<Pixels>,
    origin: gpui::Point<Pixels>,
    scale: f32,
) -> Bounds<Pixels> {
    let start = bounds.origin + origin;
    let end = start + point(bounds.size.width, bounds.size.height);
    let align = |value: Pixels| (value * scale).round() / scale;
    Bounds::from_corners(
        point(align(start.x), align(start.y)),
        point(align(end.x), align(end.y)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ghostty::{
        RenderState, SelectionDrag, SelectionGeometry, SelectionPoint, SelectionPress, Terminal,
        TerminalOptions, render::RowIterator,
    };
    use crate::widget::{PtyCommand, TerminalConfig};
    use gpui::{AppContext, Bounds, Entity, FocusHandle, TestAppContext, size};
    use gpui_component::Root;
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn terminal_spans_share_pixel_aligned_edges_at_fractional_dpi() {
        let cell_width = px(9.6);
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let first = pixel_aligned_bounds(
                Bounds::new(
                    point(cell_width, px(16.0)),
                    size(cell_width * 2.0, px(16.0)),
                ),
                point(px(176.0), px(34.0)),
                scale,
            );
            let next = pixel_aligned_bounds(
                Bounds::new(
                    point(cell_width * 3.0, px(16.0)),
                    size(cell_width, px(16.0)),
                ),
                point(px(176.0), px(34.0)),
                scale,
            );
            assert_eq!(first.right(), next.left());
            assert_eq!(first.origin.x * scale, (first.origin.x * scale).round());
        }
        let span = pixel_aligned_bounds(
            Bounds::new(point(px(9.6), px(0.0)), size(px(19.2), px(16.0))),
            point(px(0.0), px(0.0)),
            1.0,
        );
        assert_eq!(span.origin.x, px(10.0));
        assert_eq!(span.size.width, px(19.0));
    }

    struct TerminalTabFixture {
        terminal: Entity<TerminalWidget>,
        next_focus: FocusHandle,
    }

    impl Render for TerminalTabFixture {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(self.terminal.clone())
                .child(div().track_focus(&self.next_focus))
        }
    }

    #[test]
    fn bold_style_survives_box_emoji_prompt_segment() {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 32,
            rows: 4,
            max_scrollback: 100,
        })
        .expect("terminal");
        terminal.resize(32, 4, 10, 20).expect("resize");
        terminal.vt_write("📦 \u{1b}[1mrepo\u{1b}[0m".as_bytes());

        let mut render_state = RenderState::new().expect("render state");
        let snapshot = render_state.update(&terminal).expect("snapshot");
        let mut row_iterator = RowIterator::new().expect("row iterator");
        let mut cell_iterator = crate::ghostty::render::CellIterator::new().expect("cell iterator");

        let mut rows = row_iterator.update(&snapshot).expect("rows");
        let row = rows.next().expect("first row");
        let mut cells = cell_iterator.update(row).expect("cells");

        let mut letters = Vec::new();
        while let Some(cell) = cells.next() {
            let text = cell.text().expect("text");
            if text.is_empty() {
                continue;
            }

            if matches!(text.as_str(), "r" | "e" | "p" | "o") {
                letters.push((text, cell.style().expect("style").bold));
            }
        }

        assert_eq!(
            letters,
            vec![
                ("r".to_string(), true),
                ("e".to_string(), true),
                ("p".to_string(), true),
                ("o".to_string(), true),
            ]
        );
    }

    #[test]
    fn box_emoji_advances_two_columns() {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 32,
            rows: 4,
            max_scrollback: 100,
        })
        .expect("terminal");
        terminal.resize(32, 4, 10, 20).expect("resize");
        terminal.vt_write("x📦y".as_bytes());

        let mut render_state = RenderState::new().expect("render state");
        let snapshot = render_state.update(&terminal).expect("snapshot");
        let mut row_iterator = RowIterator::new().expect("row iterator");
        let mut cell_iterator = crate::ghostty::render::CellIterator::new().expect("cell iterator");

        let mut rows = row_iterator.update(&snapshot).expect("rows");
        let row = rows.next().expect("first row");
        let mut cells = cell_iterator.update(row).expect("cells");

        let mut positions = Vec::new();
        let mut col_idx = 0u16;
        while let Some(cell) = cells.next() {
            let width = cell.width().expect("width");
            let advance = width.column_advance();
            let text = cell.text().expect("text");

            if !text.is_empty() && !matches!(width, CellWidth::SpacerTail | CellWidth::SpacerHead) {
                positions.push((text, col_idx, width));
            }

            col_idx += advance;
        }

        assert_eq!(
            positions,
            vec![
                ("x".to_string(), 0, CellWidth::Narrow),
                ("📦".to_string(), 1, CellWidth::Wide),
                ("y".to_string(), 3, CellWidth::Narrow),
            ]
        );
    }

    #[test]
    fn render_row_exposes_one_selection_span() {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 12,
            rows: 2,
            max_scrollback: 100,
        })
        .expect("terminal");
        terminal.resize(12, 2, 10, 20).expect("resize");
        terminal.vt_write(b"hello world");
        terminal
            .selection_press(SelectionPress {
                point: selection_test_point(0),
                time_ns: 1,
                repeat_interval_ns: 500_000_000,
                repeat_distance: 10.0,
            })
            .expect("press");
        let mut drag_point = selection_test_point(4);
        drag_point.surface_x = 49.0;
        terminal
            .selection_drag(SelectionDrag {
                point: drag_point,
                geometry: SelectionGeometry {
                    columns: 12,
                    cell_width: 10,
                    screen_height: 40,
                },
                rectangle: false,
            })
            .expect("drag");

        let mut render_state = RenderState::new().expect("render state");
        let snapshot = render_state.update(&terminal).expect("snapshot");
        let mut row_iterator = RowIterator::new().expect("row iterator");
        let mut rows = row_iterator.update(&snapshot).expect("rows");

        assert_eq!(
            rows.next()
                .expect("first row")
                .selection()
                .expect("selection"),
            Some(crate::ghostty::render::RowSelection { start: 0, end: 4 })
        );
        assert_eq!(
            rows.next()
                .expect("second row")
                .selection()
                .expect("selection"),
            None
        );
    }

    #[test]
    fn selected_background_only_overrides_intersecting_cells() {
        let selection = Some(crate::ghostty::render::RowSelection { start: 2, end: 4 });
        let normal = RgbColor { r: 1, g: 2, b: 3 };
        let selected = RgbColor { r: 4, g: 5, b: 6 };

        assert_eq!(
            selected_background(selection, 0, 2, Some(normal), selected),
            Some(normal)
        );
        assert_eq!(
            selected_background(selection, 1, 2, Some(normal), selected),
            Some(selected)
        );
        assert_eq!(selected_background(selection, 5, 1, None, selected), None);
    }

    #[gpui::test]
    fn tab_is_forwarded_without_leaving_terminal_focus(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::widget::init(cx);
        });

        let terminal_slot = Rc::new(RefCell::new(None));
        let build_terminal_slot = Rc::clone(&terminal_slot);
        let (pty_tx, pty_rx) = crate::widget::pty::test_channel();
        let config = TerminalConfig {
            cursor_blink: false,
            ..Default::default()
        };

        let (_root, cx) = cx.add_window_view(move |window, cx| {
            let terminal = cx.new(|cx| {
                TerminalWidget::with_pty(
                    config,
                    Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    None,
                    None,
                    Some(pty_tx),
                    cx,
                )
            });
            build_terminal_slot.replace(Some(terminal.clone()));
            let fixture = cx.new(|cx| TerminalTabFixture {
                terminal,
                next_focus: cx.focus_handle().tab_stop(true),
            });
            Root::new(fixture, window, cx)
        });
        let terminal = terminal_slot
            .borrow()
            .clone()
            .expect("test terminal was created");

        cx.refresh().expect("refresh tab focus fixture");
        cx.update_window_entity(&terminal, |terminal, window, _cx| {
            terminal.request_focus(window);
        });
        while pty_rx.try_recv().is_ok() {}

        cx.simulate_keystrokes("tab");

        let terminal_is_focused = cx.update_window_entity(&terminal, |terminal, window, _cx| {
            terminal.focus_handle().is_focused(window)
        });
        assert!(terminal_is_focused);
        assert!(matches!(
            pty_rx.try_recv(),
            Ok(PtyCommand::Write(bytes)) if bytes == b"\t"
        ));
    }

    #[gpui::test]
    fn kitty_graphics_fixture_reaches_the_final_widget_render(cx: &mut TestAppContext) {
        let mut config = TerminalConfig {
            initial_cols: 20,
            initial_rows: 8,
            cursor_blink: false,
            ..Default::default()
        };
        config.theme.background = gpui::rgb(0x101010);

        let (widget, cx) = cx.add_window_view(|_window, cx| {
            TerminalWidget::with_pty(
                config,
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                None,
                None,
                None,
                cx,
            )
        });
        cx.simulate_resize(size(px(192.0), px(115.2)));
        cx.refresh().expect("refresh resized test window");
        cx.run_until_parked();
        cx.update_window_entity(&widget, |widget, window, cx| {
            widget.request_focus(window);
            widget.terminal.vt_write(
                concat!(
                    "\x1b[41mX\x1b[0m\x1b[H",
                    "\x1b_Ga=t,t=d,f=32,i=7,s=4,v=4;",
                    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                    "AAAAAAAAAAAAAAAAAAAAAA==",
                    "\x1b\\",
                    "\x1b_Ga=p,i=7,p=1,z=-1073741825,x=1,y=1,w=2,h=2,c=2,r=2;\x1b\\",
                    "\x1b[H",
                    "\x1b_Ga=p,i=7,p=2,z=-1,x=1,y=1,w=2,h=2,c=2,r=2;\x1b\\",
                    "\x1b[H",
                    "\x1b_Ga=p,i=7,p=3,z=0,x=1,y=1,w=2,h=2,c=2,r=2;\x1b\\",
                    "\x1b[H",
                    "\x1b_Ga=p,i=7,p=4,z=5,x=1,y=1,w=2,h=2,c=2,r=2;\x1b\\",
                    "\x1b[5;5H\x1b[41mX\x1b[0m"
                )
                .as_bytes(),
            );
            cx.notify();
        });
        cx.run_until_parked();

        let mut actual = String::from("before deletion\n");
        for stage in render_stage_trace() {
            actual.push_str(&stage);
            actual.push('\n');
        }
        actual.push_str(&format_fixture_bounds(
            "crop clip",
            cx.debug_bounds("kitty-graphics-clip-7-3-0")
                .unwrap_or_else(|| {
                    panic!(
                        "cropped placement clip; render trace: {:?}",
                        render_stage_trace()
                    )
                }),
        ));
        actual.push('\n');
        actual.push_str(&format_fixture_bounds(
            "full image",
            cx.debug_bounds("kitty-graphics-image-7-3-0")
                .expect("cropped placement image"),
        ));
        actual.push('\n');

        cx.update_entity(&widget, |widget, cx| {
            widget.terminal.vt_write(b"\x1b_Ga=d,d=A\x1b\\");
            cx.notify();
        });
        cx.refresh().expect("refresh deleted graphics");
        cx.run_until_parked();

        actual.push_str("after deletion\n");
        for stage in render_stage_trace() {
            actual.push_str(&stage);
            actual.push('\n');
        }
        actual.push_str(&format!(
            "crop clip present={}\n",
            cx.debug_bounds("kitty-graphics-clip-7-3-0").is_some()
        ));

        assert_eq!(
            actual,
            include_str!("fixtures/kitty-graphics-render.golden").replace("\r\n", "\n")
        );
    }

    #[gpui::test]
    fn native_text_commits_once_and_preedit_never_reaches_the_pty(cx: &mut TestAppContext) {
        use gpui::EntityInputHandler;
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::widget::init(cx);
        });
        let (pty_tx, pty_rx) = crate::widget::pty::test_channel();
        let slot = Rc::new(RefCell::new(None));
        let build_slot = slot.clone();
        let (_root, cx) = cx.add_window_view(move |window, cx| {
            let widget = cx.new(|cx| {
                TerminalWidget::with_pty(
                    TerminalConfig {
                        cursor_blink: false,
                        ..Default::default()
                    },
                    Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    None,
                    None,
                    Some(pty_tx),
                    cx,
                )
            });
            build_slot.replace(Some(widget.clone()));
            let fixture = cx.new(|cx| TerminalTabFixture {
                terminal: widget,
                next_focus: cx.focus_handle(),
            });
            Root::new(fixture, window, cx)
        });
        let widget = slot.borrow().clone().unwrap();
        cx.update_window_entity(&widget, |widget, window, _| {
            window.activate_window();
            widget.request_focus(window);
        });
        cx.refresh().unwrap();
        while pty_rx.try_recv().is_ok() {}
        cx.simulate_keystrokes("a");
        let writes = pty_rx
            .try_iter()
            .filter_map(|command| match command {
                PtyCommand::Write(bytes) => Some(bytes),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(writes, vec![b"a".to_vec()]);
        cx.update_window_entity(&widget, |widget, window, cx| {
            widget.replace_and_mark_text_in_range(None, "日本😀", Some(2..2), window, cx);
            assert!(pty_rx.try_recv().is_err());
            let candidate = widget.input_cursor_bounds().unwrap();
            assert!(widget.layout_bounds.unwrap().contains(&candidate.origin));
            widget.unmark_text(window, cx);
            assert!(pty_rx.try_recv().is_err());
            widget.replace_and_mark_text_in_range(None, "日本", None, window, cx);
            widget.replace_text_in_range(None, "日本", window, cx);
            assert!(widget.preedit.is_empty());
        });
        let writes = pty_rx
            .try_iter()
            .filter_map(|command| match command {
                PtyCommand::Write(bytes) => Some(bytes),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(writes, vec!["日本".as_bytes().to_vec()]);

        cx.update_window_entity(&widget, |widget, window, cx| {
            widget.terminal.vt_write(b"\x1b[>11u");
            let key = gpui::Keystroke::parse("a").unwrap();
            widget.handle_key_down(
                &KeyDownEvent {
                    keystroke: key.clone(),
                    is_held: false,
                },
                window,
                cx,
            );
            assert!(pty_rx.try_recv().is_err());
            widget.replace_text_in_range(None, "a", window, cx);
            widget.handle_key_up(&KeyUpEvent { keystroke: key }, window, cx);
        });
        let writes = pty_rx
            .try_iter()
            .filter_map(|command| match command {
                PtyCommand::Write(bytes) => Some(bytes),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(writes, vec![b"\x1b[97u".to_vec(), b"\x1b[97;1:3u".to_vec()]);
        cx.update_window_entity(&widget, |widget, window, cx| {
            widget.replace_and_mark_text_in_range(None, "", Some(0..0), window, cx);
            assert_eq!(widget.marked_text_range(window, cx), Some(0..0));
            widget.replace_and_mark_text_in_range(None, "日本", None, window, cx);
            window.blur();
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update_window_entity(&widget, |widget, window, _| {
            assert!(!widget.composing);
            assert!(widget.preedit.is_empty());
            widget.request_focus(window);
        });
        assert!(pty_rx.try_recv().is_err());

        cx.update_window_entity(&widget, |widget, window, cx| widget.open_search(window, cx));
        cx.refresh().unwrap();
        cx.simulate_input("日本😀");
        cx.run_until_parked();
        cx.update_window_entity(&widget, |widget, _, _| {
            assert_eq!(widget.search.as_ref().unwrap().query, "日本😀");
        });
        assert!(
            pty_rx
                .try_iter()
                .all(|command| !matches!(command, PtyCommand::Write(_)))
        );
        cx.simulate_keystrokes("escape");
        cx.update_window_entity(&widget, |widget, _, _| assert!(widget.search.is_none()));
    }

    #[gpui::test]
    fn configured_font_size_changes_grid_and_candidate_geometry(cx: &mut TestAppContext) {
        let (widget, cx) = cx.add_window_view(|_, cx| {
            TerminalWidget::with_pty(
                TerminalConfig {
                    cursor_blink: false,
                    ..Default::default()
                },
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                None,
                None,
                None,
                cx,
            )
        });
        cx.simulate_resize(size(px(800.0), px(600.0)));
        cx.refresh().unwrap();
        cx.run_until_parked();
        let (small_cells, small_grid) = cx.update_window_entity(&widget, |widget, _, cx| {
            let metrics = (widget.cell_size, widget.size);
            widget.config.font_size_px = 32.0;
            cx.notify();
            metrics
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update_window_entity(&widget, |widget, _, _| {
            assert!(widget.cell_size.0 > small_cells.0);
            assert!(widget.cell_size.1 > small_cells.1);
            assert!(widget.size.0 < small_grid.0 && widget.size.1 < small_grid.1);
            assert_eq!(
                widget.input_cursor_bounds().unwrap().size,
                size(widget.cell_size.0, widget.cell_size.1)
            );
        });
    }

    fn format_fixture_bounds(label: &str, bounds: Bounds<Pixels>) -> String {
        let left = f32::from(bounds.origin.x);
        let top = f32::from(bounds.origin.y);
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        format!("{label}=({left:.3},{top:.3}) {width:.3}x{height:.3}")
    }

    fn selection_test_point(column: u16) -> SelectionPoint {
        SelectionPoint {
            column,
            row: 0,
            surface_x: f64::from(column) * 10.0 + 1.0,
            surface_y: 1.0,
        }
    }
}
#[cfg(test)]
mod presentation_tests {
    use super::*;
    use crate::widget::{CursorStyle, PtyEvent, TerminalConfig};
    use gpui::TestAppContext;
    use std::rc::Rc;
    #[gpui::test]
    fn capture_keeps_the_painted_revision_after_a_new_commit(cx: &mut TestAppContext) {
        let (widget, cx) = cx.add_window_view(|_, cx| {
            TerminalWidget::with_pty(
                TerminalConfig {
                    cursor_blink: false,
                    ..Default::default()
                },
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                None,
                None,
                None,
                cx,
            )
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update_window_entity(&widget, |widget, window, cx| {
            widget.apply_pty_event(PtyEvent::Output(b"\x1b[2J\x1b[Hpainted".to_vec()), cx);
            widget.publish_terminal(cx);
            let _ = widget.paint_terminal(window, cx, false);
            let live_before = widget.presentation(window, false, true);
            let _ = widget.offscreen_element(window, cx);
            let before = widget.presentation(window, true, true);
            widget.apply_pty_event(
                PtyEvent::Output(b"\x1b[2J\x1b[Hnewer\x1b[?25l".to_vec()),
                cx,
            );
            widget.publish_terminal(cx);
            assert_ne!(
                widget.committed.as_ref().unwrap().output_seq,
                widget.offscreen_painted.as_ref().unwrap().frame.output_seq
            );
            let retained = widget.presentation(window, true, true);
            assert_eq!(before.state["output_seq"], retained.state["output_seq"]);
            assert!(
                retained
                    .source
                    .unwrap()
                    .rows
                    .iter()
                    .any(|r| r.text.contains("painted"))
            );
            let _ = widget.offscreen_element(window, cx);
            let newest = widget.presentation(window, true, true);
            assert_eq!(newest.state["output_seq"], widget.output_seq.to_string());
            assert_eq!(
                newest.state["terminal_status"]["value"]["cursor_visible"],
                false
            );
            assert!(
                newest
                    .source
                    .unwrap()
                    .rows
                    .iter()
                    .any(|r| r.text.contains("newer"))
            );
            let live = widget.presentation(window, false, true);
            assert_eq!(live.state["output_seq"], live_before.state["output_seq"]);
            assert!(
                live.source
                    .unwrap()
                    .rows
                    .iter()
                    .any(|r| r.text.contains("painted"))
            );
        });
    }
    #[gpui::test]
    fn explicit_pane_focus_does_not_reselect_the_old_focus(cx: &mut TestAppContext) {
        use crate::split::{PaneId, Split, SplitAxis, SplitNode};
        let first = PaneId::allocate();
        let second = PaneId::allocate();
        let (split, cx) = cx.add_window_view(|window, cx| {
            let make = |cx: &mut gpui::App| {
                cx.new(|cx| {
                    TerminalWidget::with_pty(
                        TerminalConfig {
                            cursor_blink: false,
                            ..Default::default()
                        },
                        Arc::new(std::sync::atomic::AtomicBool::new(false)),
                        None,
                        None,
                        None,
                        cx,
                    )
                })
            };
            let first_widget = make(cx);
            let second_widget = make(cx);
            first_widget.update(cx, |widget, _| widget.request_focus(window));
            Split::from_restored(
                SplitNode::Branch {
                    axis: SplitAxis::Horizontal,
                    ratio: 0.5,
                    first: Box::new(SplitNode::Leaf { pane_id: first }),
                    second: Box::new(SplitNode::Leaf { pane_id: second }),
                },
                vec![
                    (
                        first,
                        first_widget,
                        crate::profile::ProfileId::new("fixture").unwrap(),
                    ),
                    (
                        second,
                        second_widget,
                        crate::profile::ProfileId::new("fixture").unwrap(),
                    ),
                ],
                first,
            )
            .unwrap()
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update_window_entity(&split, |split, window, cx| {
            assert!(split.focus_target(second, None, window, cx));
            assert_eq!(split.active_pane_id(), second);
            assert!(
                split
                    .terminal(second)
                    .unwrap()
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            );
        });
    }
    #[gpui::test]
    fn protocol_cursor_and_synchronized_repaints_use_the_committed_frame(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::widget::init(cx);
        });
        let (widget, cx) = cx.add_window_view(|_, cx| {
            TerminalWidget::with_pty(
                TerminalConfig {
                    cursor_style: CursorStyle::Underline,
                    cursor_blink: false,
                    ..Default::default()
                },
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                None,
                None,
                None,
                cx,
            )
        });
        cx.simulate_resize(size(px(600.), px(400.)));
        cx.refresh().unwrap();
        cx.run_until_parked();
        for (bytes, shape, blinking, visible) in [
            (
                &b"\x1b[?25h\x1b[12;4H"[..],
                crate::ghostty::render::CursorShape::Underline,
                false,
                true,
            ),
            (
                &b"\x1b[?25l\x1b[2;7Hlabel"[..],
                crate::ghostty::render::CursorShape::Underline,
                false,
                false,
            ),
            (
                &b"\x1b[5;19Hspinner"[..],
                crate::ghostty::render::CursorShape::Underline,
                false,
                false,
            ),
            (
                &b"\x1b[?25h\x1b[2 q"[..],
                crate::ghostty::render::CursorShape::Block,
                false,
                true,
            ),
            (
                &b"\x1b[5 q"[..],
                crate::ghostty::render::CursorShape::Bar,
                true,
                true,
            ),
            (
                &b"\x1b[?12l"[..],
                crate::ghostty::render::CursorShape::Bar,
                false,
                true,
            ),
            (
                &b"\x1b[0 q"[..],
                crate::ghostty::render::CursorShape::Underline,
                false,
                true,
            ),
            (
                &b"\x1b[?1049h\x1b[3 q"[..],
                crate::ghostty::render::CursorShape::Underline,
                true,
                true,
            ),
            (
                &b"\x1b[?1049l\x1bc"[..],
                crate::ghostty::render::CursorShape::Underline,
                false,
                true,
            ),
        ] {
            cx.update_window_entity(&widget, |widget, window, cx| {
                widget.request_focus(window);
                widget.cursor_blink_phase = true;
                widget.apply_pty_event(PtyEvent::Output(bytes.to_vec()), cx);
                cx.notify();
            });
            cx.run_until_parked();
            cx.update_window_entity(&widget, |widget, _, _| {
                let frame = widget.committed.as_ref().unwrap();
                assert_eq!(
                    (
                        frame.cursor.shape,
                        frame.cursor.blinking,
                        frame.cursor.visible
                    ),
                    (shape, blinking, visible)
                );
            });
            assert!(
                render_stage_trace().contains(&format!("cursor count={}", usize::from(visible)))
            );
        }
        let before =
            cx.update_window_entity(&widget, |widget, _, _| widget.committed.clone().unwrap());
        cx.update_window_entity(&widget, |widget, _, cx| {
            widget.apply_pty_event(
                PtyEvent::Output(b"\x1b[?2026h\x1b[2J\x1b[7;19Hpartial".to_vec()),
                cx,
            );
            widget.publish_terminal(cx);
            widget.cursor_blink_phase = false;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update_window_entity(&widget, |widget, window, cx| {
            let anchor = widget.input_cursor_bounds().unwrap();
            widget.preedit = "日本".into();
            widget.composing = true;
            window.blur();
            let _ = widget.offscreen_element(window, cx);
            assert!(Rc::ptr_eq(&before, widget.committed.as_ref().unwrap()));
            assert_eq!(anchor, widget.input_cursor_bounds().unwrap());
            widget.search = Some(Default::default());
            cx.notify();
        });
        cx.run_until_parked();
        cx.update_window_entity(&widget, |widget, _, _| {
            assert!(Rc::ptr_eq(&before, widget.committed.as_ref().unwrap()))
        });
    }
}
