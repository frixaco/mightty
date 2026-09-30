//! Native composition is local UI state; only committed text reaches Ghostty.
use super::{TerminalWidget, ViewportScroll};
use gpui::{
    Bounds, Context, EntityInputHandler, Keystroke, Pixels, Point, UTF16Selection, Window, point,
    size,
};
use std::ops::Range;

pub(super) fn uses_native_text(key: &Keystroke) -> bool {
    !key.modifiers.control
        && !key.modifiers.alt
        && !key.modifiers.platform
        && (key.key == "space" || key.key.chars().count() == 1 || key.key_char.is_some())
}

// Native offsets are UTF-16, not UTF-8 byte positions. Expand partial surrogate
// ranges to scalar boundaries before slicing or replacing local preedit text.
fn byte_range(text: &str, range: Range<usize>) -> Range<usize> {
    let mut units = 0;
    let mut start = text.len();
    let mut end = text.len();
    for (byte, ch) in text.char_indices() {
        if units <= range.start && range.start < units + ch.len_utf16() {
            start = byte;
        }
        if units < range.end && range.end <= units + ch.len_utf16() {
            end = byte + ch.len_utf8();
        }
        units += ch.len_utf16();
    }
    if range.start == range.end {
        end = start;
    }
    start..end.max(start)
}

impl TerminalWidget {
    pub(super) fn input_cursor_bounds(&mut self) -> Option<Bounds<Pixels>> {
        let origin = self.layout_bounds?.origin;
        let snapshot = self.render_state.update(&self.terminal).ok()?;
        let cursor = snapshot.cursor_viewport().ok()??;
        Some(Bounds::new(
            origin
                + point(
                    self.cell_size.0 * f32::from(cursor.x),
                    self.cell_size.1 * f32::from(cursor.y),
                ),
            size(self.cell_size.0, self.cell_size.1),
        ))
    }
}

impl EntityInputHandler for TerminalWidget {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let bytes = byte_range(&self.preedit, range);
        let start = self.preedit[..bytes.start].encode_utf16().count();
        let text = &self.preedit[bytes];
        *adjusted = Some(start..start + text.encode_utf16().count());
        Some(text.to_string())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.preedit_selection.clone(),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.composing
            .then(|| 0..self.preedit.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.preedit.clear();
        self.composing = false;
        self.preedit_selection = 0..0;
        self.pending_text_key = None;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preedit.clear();
        self.composing = false;
        self.preedit_selection = 0..0;
        let pending = self.pending_text_key.take();
        if self.search.is_some() || self.pending_paste.is_some() || text.is_empty() {
            cx.notify();
            return;
        }
        // A normal printable key keeps its physical identity/repeat state for
        // extended keyboard protocols. An IME commit has no physical identity.
        if let Some((action, mut key)) = pending {
            key.key_char = Some(text.to_string());
            self.send_encoded_key(action, &key, cx);
        } else {
            match self.key_encoder.encode_text(&self.terminal, text) {
                Ok(bytes) if !bytes.is_empty() => {
                    self.terminal.scroll_viewport(ViewportScroll::Bottom);
                    let _ = self.terminal.clear_selection();
                    self.send_pty_command(super::PtyCommand::Write(bytes));
                }
                Ok(_) => {}
                Err(error) => eprintln!("Failed to encode committed text: {error}"),
            }
            self.reset_cursor_blink(cx);
        }
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search.is_some() || self.pending_paste.is_some() {
            return;
        }
        self.pending_text_key = None;
        self.composing = true;
        let bytes = byte_range(
            &self.preedit,
            range.unwrap_or(0..self.preedit.encode_utf16().count()),
        );
        self.preedit.replace_range(bytes, text);
        let length = self.preedit.encode_utf16().count();
        let selection = selection.unwrap_or(length..length);
        self.preedit_selection = selection.start.min(length)..selection.end.min(length);
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.input_cursor_bounds()
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.input_cursor_bounds()
            .filter(|bounds| bounds.contains(&point))
            .map(|_| self.preedit_selection.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_ranges_preserve_utf8_and_surrogate_boundaries() {
        let text = "a😀日本";
        assert_eq!(&text[byte_range(text, 1..3)], "😀");
        assert_eq!(&text[byte_range(text, 2..3)], "😀");
        assert_eq!(&text[byte_range(text, 3..5)], "日本");
        assert_eq!(&text[byte_range(text, 5..5)], "");
    }
    #[test]
    fn shortcuts_and_control_keys_keep_key_encoding() {
        assert!(uses_native_text(&Keystroke::parse("shift-a").unwrap()));
        assert!(uses_native_text(&Keystroke::parse("space").unwrap()));
        for key in ["ctrl-a", "alt-a", "enter", "backspace", "up", "tab"] {
            assert!(!uses_native_text(&Keystroke::parse(key).unwrap()), "{key}");
        }
    }
}
