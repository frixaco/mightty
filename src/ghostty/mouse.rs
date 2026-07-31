use std::marker::PhantomData;
use std::mem::{MaybeUninit, size_of};
use std::ptr::NonNull;
use std::rc::Rc;

use crate::ghostty::error::{from_result, from_result_with_len};
use crate::ghostty::key::Mods;
use crate::ghostty::{Error, Result, Terminal, ffi};

/// Encoder for normalized pointer events requested by a terminal application.
pub struct Encoder {
    raw: NonNull<ffi::MouseEncoderImpl>,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

/// A reusable normalized pointer event.
pub struct Event {
    raw: NonNull<ffi::MouseEventImpl>,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

/// Surface geometry used to convert pixel positions to terminal cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub screen_width: u32,
    pub screen_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
    pub padding_right: u32,
    pub padding_left: u32,
}

impl Encoder {
    pub fn new() -> Result<Self> {
        let mut raw = std::ptr::null_mut();
        let result = unsafe { ffi::ghostty_mouse_encoder_new(std::ptr::null(), &raw mut raw) };
        from_result(result)?;
        Ok(Self {
            raw: NonNull::new(raw).ok_or(Error::InvalidValue)?,
            _not_send_or_sync: PhantomData,
        })
    }

    pub fn set_options_from_terminal(&mut self, terminal: &Terminal) -> &mut Self {
        unsafe {
            ffi::ghostty_mouse_encoder_setopt_from_terminal(self.raw.as_ptr(), terminal.as_raw());
        }
        self
    }

    pub fn set_geometry(&mut self, geometry: Geometry) -> &mut Self {
        let raw = ffi::MouseEncoderSize {
            size: size_of::<ffi::MouseEncoderSize>(),
            screen_width: geometry.screen_width,
            screen_height: geometry.screen_height,
            cell_width: geometry.cell_width,
            cell_height: geometry.cell_height,
            padding_top: geometry.padding_top,
            padding_bottom: geometry.padding_bottom,
            padding_right: geometry.padding_right,
            padding_left: geometry.padding_left,
        };
        self.set_option(ffi::MouseEncoderOption::SIZE, &raw);
        self
    }

    pub fn set_any_button_pressed(&mut self, pressed: bool) -> &mut Self {
        self.set_option(ffi::MouseEncoderOption::ANY_BUTTON_PRESSED, &pressed);
        self
    }

    /// Append the encoded event bytes to `output`.
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

    fn set_option<T>(&mut self, option: ffi::MouseEncoderOption::Type, value: &T) {
        unsafe {
            ffi::ghostty_mouse_encoder_setopt(
                self.raw.as_ptr(),
                option,
                std::ptr::from_ref(value).cast(),
            );
        }
    }

    fn encode_to_uninit(&mut self, event: &Event, output: &mut [MaybeUninit<u8>]) -> Result<usize> {
        let mut written = 0;
        let result = unsafe {
            ffi::ghostty_mouse_encoder_encode(
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
        let result = unsafe { ffi::ghostty_mouse_event_new(std::ptr::null(), &raw mut raw) };
        from_result(result)?;
        Ok(Self {
            raw: NonNull::new(raw).ok_or(Error::InvalidValue)?,
            _not_send_or_sync: PhantomData,
        })
    }

    pub fn set_action(&mut self, action: Action) -> &mut Self {
        unsafe {
            ffi::ghostty_mouse_event_set_action(self.raw.as_ptr(), action.as_raw());
        }
        self
    }

    pub fn set_button(&mut self, button: Option<Button>) -> &mut Self {
        unsafe {
            match button {
                Some(button) => {
                    ffi::ghostty_mouse_event_set_button(self.raw.as_ptr(), button.as_raw())
                }
                None => ffi::ghostty_mouse_event_clear_button(self.raw.as_ptr()),
            }
        }
        self
    }

    pub fn set_mods(&mut self, mods: Mods) -> &mut Self {
        unsafe {
            ffi::ghostty_mouse_event_set_mods(self.raw.as_ptr(), mods.bits());
        }
        self
    }

    pub fn set_position(&mut self, x: f32, y: f32) -> &mut Self {
        unsafe {
            ffi::ghostty_mouse_event_set_position(self.raw.as_ptr(), ffi::MousePosition { x, y });
        }
        self
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_mouse_encoder_free(self.raw.as_ptr());
        }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_mouse_event_free(self.raw.as_ptr());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    Motion,
}

impl Action {
    fn as_raw(self) -> ffi::MouseAction::Type {
        match self {
            Self::Press => ffi::MouseAction::PRESS,
            Self::Release => ffi::MouseAction::RELEASE,
            Self::Motion => ffi::MouseAction::MOTION,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Ten,
    Eleven,
}

impl Button {
    fn as_raw(self) -> ffi::MouseButton::Type {
        match self {
            Self::Left => ffi::MouseButton::LEFT,
            Self::Right => ffi::MouseButton::RIGHT,
            Self::Middle => ffi::MouseButton::MIDDLE,
            Self::Four => ffi::MouseButton::FOUR,
            Self::Five => ffi::MouseButton::FIVE,
            Self::Six => ffi::MouseButton::SIX,
            Self::Seven => ffi::MouseButton::SEVEN,
            Self::Eight => ffi::MouseButton::EIGHT,
            Self::Nine => ffi::MouseButton::NINE,
            Self::Ten => ffi::MouseButton::TEN,
            Self::Eleven => ffi::MouseButton::ELEVEN,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ghostty::TerminalOptions;

    #[test]
    fn encodes_sgr_mouse_input_from_terminal_modes() {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 80,
            rows: 24,
            max_scrollback: 0,
        })
        .unwrap();
        terminal.vt_write(b"\x1b[?1000h\x1b[?1006h");

        let mut encoder = Encoder::new().unwrap();
        let mut event = Event::new().unwrap();
        let mut output = Vec::new();
        event
            .set_action(Action::Press)
            .set_button(Some(Button::Left))
            .set_mods(Mods::empty())
            .set_position(5.0, 10.0);
        encoder
            .set_options_from_terminal(&terminal)
            .set_geometry(Geometry {
                screen_width: 800,
                screen_height: 480,
                cell_width: 10,
                cell_height: 20,
                padding_top: 0,
                padding_bottom: 0,
                padding_right: 0,
                padding_left: 0,
            })
            .encode_to_vec(&event, &mut output)
            .unwrap();

        assert_eq!(output, b"\x1b[<0;1;1M");
    }

    #[test]
    fn emits_nothing_when_mouse_tracking_is_disabled() {
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
            .set_action(Action::Motion)
            .set_button(None)
            .set_mods(Mods::empty())
            .set_position(5.0, 10.0);
        encoder
            .set_options_from_terminal(&terminal)
            .encode_to_vec(&event, &mut output)
            .unwrap();

        assert!(output.is_empty());
    }
}
