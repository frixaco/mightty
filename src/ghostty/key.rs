use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ops::{BitOr, BitOrAssign};
use std::ptr::NonNull;
use std::rc::Rc;

use crate::ghostty::error::{from_result, from_result_with_len};
use crate::ghostty::{Error, Result, Terminal, ffi};

/// Encoder for turning normalized key events into terminal input bytes.
pub struct Encoder {
    raw: NonNull<ffi::KeyEncoderImpl>,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

/// A reusable normalized key event.
pub struct Event {
    raw: NonNull<ffi::KeyEventImpl>,
    text: Option<String>,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

impl Encoder {
    pub fn new() -> Result<Self> {
        let mut raw = std::ptr::null_mut();
        let result = unsafe { ffi::ghostty_key_encoder_new(std::ptr::null(), &raw mut raw) };
        from_result(result)?;
        Ok(Self {
            raw: NonNull::new(raw).ok_or(Error::InvalidValue)?,
            _not_send_or_sync: PhantomData,
        })
    }

    pub fn set_options_from_terminal(&mut self, terminal: &Terminal) -> &mut Self {
        unsafe {
            ffi::ghostty_key_encoder_setopt_from_terminal(self.raw.as_ptr(), terminal.as_raw());
        }
        self
    }

    /// Append this event's encoded bytes to `output`.
    pub fn encode_to_vec(&mut self, event: &Event, output: &mut Vec<u8>) -> Result<()> {
        let written = match self.encode_to_uninit(event, output.spare_capacity_mut()) {
            Ok(written) => written,
            Err(Error::OutOfSpace { required }) => {
                output.reserve(required);
                self.encode_to_uninit(event, output.spare_capacity_mut())?
            }
            Err(error) => return Err(error),
        };
        if written > output.spare_capacity_mut().len() {
            return Err(Error::InvalidValue);
        }
        unsafe {
            output.set_len(output.len() + written);
        }
        Ok(())
    }

    fn encode_to_uninit(&mut self, event: &Event, output: &mut [MaybeUninit<u8>]) -> Result<usize> {
        let mut written = 0;
        let result = unsafe {
            ffi::ghostty_key_encoder_encode(
                self.raw.as_ptr(),
                event.raw.as_ptr(),
                output.as_mut_ptr().cast(),
                output.len(),
                &raw mut written,
            )
        };
        from_result_with_len(result, written)
    }
}

impl Event {
    pub fn new() -> Result<Self> {
        let mut raw = std::ptr::null_mut();
        let result = unsafe { ffi::ghostty_key_event_new(std::ptr::null(), &raw mut raw) };
        from_result(result)?;
        Ok(Self {
            raw: NonNull::new(raw).ok_or(Error::InvalidValue)?,
            text: None,
            _not_send_or_sync: PhantomData,
        })
    }

    pub fn set_action(&mut self, action: Action) -> &mut Self {
        unsafe {
            ffi::ghostty_key_event_set_action(self.raw.as_ptr(), action.as_raw());
        }
        self
    }

    pub fn set_key(&mut self, key: Key) -> &mut Self {
        unsafe {
            ffi::ghostty_key_event_set_key(self.raw.as_ptr(), key.as_raw());
        }
        self
    }

    pub fn set_mods(&mut self, mods: Mods) -> &mut Self {
        unsafe {
            ffi::ghostty_key_event_set_mods(self.raw.as_ptr(), mods.bits());
        }
        self
    }

    pub fn set_consumed_mods(&mut self, mods: Mods) -> &mut Self {
        unsafe {
            ffi::ghostty_key_event_set_consumed_mods(self.raw.as_ptr(), mods.bits());
        }
        self
    }

    pub fn set_composing(&mut self, composing: bool) -> &mut Self {
        unsafe {
            ffi::ghostty_key_event_set_composing(self.raw.as_ptr(), composing);
        }
        self
    }

    pub fn set_utf8<S: Into<String>>(&mut self, text: Option<S>) -> &mut Self {
        self.text = text.map(Into::into);
        let (pointer, len) = self.text.as_ref().map_or((std::ptr::null(), 0), |text| {
            (text.as_ptr().cast(), text.len())
        });
        unsafe {
            ffi::ghostty_key_event_set_utf8(self.raw.as_ptr(), pointer, len);
        }
        self
    }

    pub fn set_unshifted_codepoint(&mut self, codepoint: char) -> &mut Self {
        unsafe {
            ffi::ghostty_key_event_set_unshifted_codepoint(self.raw.as_ptr(), u32::from(codepoint));
        }
        self
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_key_encoder_free(self.raw.as_ptr());
        }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_key_event_free(self.raw.as_ptr());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    Repeat,
}

impl Action {
    fn as_raw(self) -> ffi::KeyAction::Type {
        match self {
            Self::Press => ffi::KeyAction::PRESS,
            Self::Release => ffi::KeyAction::RELEASE,
            Self::Repeat => ffi::KeyAction::REPEAT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Unidentified,
    Backquote,
    Backslash,
    BracketLeft,
    BracketRight,
    Comma,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Equal,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Minus,
    Period,
    Quote,
    Semicolon,
    Slash,
    Backspace,
    Enter,
    Space,
    Tab,
    Delete,
    End,
    Home,
    Insert,
    PageDown,
    PageUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Escape,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl Key {
    fn as_raw(self) -> ffi::Key::Type {
        match self {
            Self::Unidentified => ffi::Key::UNIDENTIFIED,
            Self::Backquote => ffi::Key::BACKQUOTE,
            Self::Backslash => ffi::Key::BACKSLASH,
            Self::BracketLeft => ffi::Key::BRACKET_LEFT,
            Self::BracketRight => ffi::Key::BRACKET_RIGHT,
            Self::Comma => ffi::Key::COMMA,
            Self::Digit0 => ffi::Key::DIGIT_0,
            Self::Digit1 => ffi::Key::DIGIT_1,
            Self::Digit2 => ffi::Key::DIGIT_2,
            Self::Digit3 => ffi::Key::DIGIT_3,
            Self::Digit4 => ffi::Key::DIGIT_4,
            Self::Digit5 => ffi::Key::DIGIT_5,
            Self::Digit6 => ffi::Key::DIGIT_6,
            Self::Digit7 => ffi::Key::DIGIT_7,
            Self::Digit8 => ffi::Key::DIGIT_8,
            Self::Digit9 => ffi::Key::DIGIT_9,
            Self::Equal => ffi::Key::EQUAL,
            Self::A => ffi::Key::A,
            Self::B => ffi::Key::B,
            Self::C => ffi::Key::C,
            Self::D => ffi::Key::D,
            Self::E => ffi::Key::E,
            Self::F => ffi::Key::F,
            Self::G => ffi::Key::G,
            Self::H => ffi::Key::H,
            Self::I => ffi::Key::I,
            Self::J => ffi::Key::J,
            Self::K => ffi::Key::K,
            Self::L => ffi::Key::L,
            Self::M => ffi::Key::M,
            Self::N => ffi::Key::N,
            Self::O => ffi::Key::O,
            Self::P => ffi::Key::P,
            Self::Q => ffi::Key::Q,
            Self::R => ffi::Key::R,
            Self::S => ffi::Key::S,
            Self::T => ffi::Key::T,
            Self::U => ffi::Key::U,
            Self::V => ffi::Key::V,
            Self::W => ffi::Key::W,
            Self::X => ffi::Key::X,
            Self::Y => ffi::Key::Y,
            Self::Z => ffi::Key::Z,
            Self::Minus => ffi::Key::MINUS,
            Self::Period => ffi::Key::PERIOD,
            Self::Quote => ffi::Key::QUOTE,
            Self::Semicolon => ffi::Key::SEMICOLON,
            Self::Slash => ffi::Key::SLASH,
            Self::Backspace => ffi::Key::BACKSPACE,
            Self::Enter => ffi::Key::ENTER,
            Self::Space => ffi::Key::SPACE,
            Self::Tab => ffi::Key::TAB,
            Self::Delete => ffi::Key::DELETE,
            Self::End => ffi::Key::END,
            Self::Home => ffi::Key::HOME,
            Self::Insert => ffi::Key::INSERT,
            Self::PageDown => ffi::Key::PAGE_DOWN,
            Self::PageUp => ffi::Key::PAGE_UP,
            Self::ArrowDown => ffi::Key::ARROW_DOWN,
            Self::ArrowLeft => ffi::Key::ARROW_LEFT,
            Self::ArrowRight => ffi::Key::ARROW_RIGHT,
            Self::ArrowUp => ffi::Key::ARROW_UP,
            Self::Escape => ffi::Key::ESCAPE,
            Self::F1 => ffi::Key::F1,
            Self::F2 => ffi::Key::F2,
            Self::F3 => ffi::Key::F3,
            Self::F4 => ffi::Key::F4,
            Self::F5 => ffi::Key::F5,
            Self::F6 => ffi::Key::F6,
            Self::F7 => ffi::Key::F7,
            Self::F8 => ffi::Key::F8,
            Self::F9 => ffi::Key::F9,
            Self::F10 => ffi::Key::F10,
            Self::F11 => ffi::Key::F11,
            Self::F12 => ffi::Key::F12,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods(u16);

impl Mods {
    pub const SHIFT: Self = Self(ffi::MODS_SHIFT);
    pub const CTRL: Self = Self(ffi::MODS_CTRL);
    pub const ALT: Self = Self(ffi::MODS_ALT);
    pub const SUPER: Self = Self(ffi::MODS_SUPER);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub(crate) const fn bits(self) -> u16 {
        self.0
    }
}

impl BitOr for Mods {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Mods {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ghostty::TerminalOptions;

    #[test]
    fn encodes_text_and_terminal_navigation_keys() {
        let terminal = Terminal::new(TerminalOptions {
            cols: 80,
            rows: 24,
            max_scrollback: 0,
        })
        .unwrap();
        let mut encoder = Encoder::new().unwrap();
        let mut event = Event::new().unwrap();
        let mut output = Vec::new();

        event
            .set_action(Action::Press)
            .set_key(Key::A)
            .set_mods(Mods::empty())
            .set_consumed_mods(Mods::empty())
            .set_unshifted_codepoint('a')
            .set_utf8(Some("a"))
            .set_composing(false);
        encoder
            .set_options_from_terminal(&terminal)
            .encode_to_vec(&event, &mut output)
            .unwrap();
        assert_eq!(output, b"a");

        output.clear();
        event
            .set_key(Key::ArrowUp)
            .set_unshifted_codepoint('\0')
            .set_utf8::<String>(None);
        encoder.encode_to_vec(&event, &mut output).unwrap();
        assert_eq!(output, b"\x1b[A");
    }

    #[test]
    fn grows_an_existing_output_buffer_before_appending() {
        let terminal = Terminal::new(TerminalOptions {
            cols: 80,
            rows: 24,
            max_scrollback: 0,
        })
        .unwrap();
        let mut encoder = Encoder::new().unwrap();
        let mut event = Event::new().unwrap();
        let text = "x".repeat(128);
        let mut output = vec![b'p'; 32];
        output.shrink_to_fit();

        event
            .set_action(Action::Press)
            .set_key(Key::A)
            .set_mods(Mods::empty())
            .set_consumed_mods(Mods::empty())
            .set_unshifted_codepoint('a')
            .set_utf8(Some(text.as_str()))
            .set_composing(false);
        encoder
            .set_options_from_terminal(&terminal)
            .encode_to_vec(&event, &mut output)
            .unwrap();

        assert_eq!(&output[..32], &[b'p'; 32]);
        assert_eq!(&output[32..], text.as_bytes());
    }
}
