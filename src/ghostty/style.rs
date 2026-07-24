use crate::ghostty::{Error, Result, ffi};

/// An RGB color with eight bits per channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// A complete 256-color terminal palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette(pub [RgbColor; 256]);

/// Text underline style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Underline {
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// Cell styling needed by mightty's renderer and feedback capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub inverse: bool,
    pub strikethrough: bool,
    pub underline: Underline,
}

impl Palette {
    pub(crate) fn into_raw(self) -> [ffi::ColorRgb; 256] {
        self.0.map(Into::into)
    }
}

impl Style {
    pub(crate) fn from_raw(value: ffi::Style) -> Result<Self> {
        Ok(Self {
            bold: value.bold,
            italic: value.italic,
            inverse: value.inverse,
            strikethrough: value.strikethrough,
            underline: Underline::from_raw(value.underline)?,
        })
    }
}

impl Underline {
    fn from_raw(value: i32) -> Result<Self> {
        match value {
            ffi::SgrUnderline::NONE => Ok(Self::None),
            ffi::SgrUnderline::SINGLE => Ok(Self::Single),
            ffi::SgrUnderline::DOUBLE => Ok(Self::Double),
            ffi::SgrUnderline::CURLY => Ok(Self::Curly),
            ffi::SgrUnderline::DOTTED => Ok(Self::Dotted),
            ffi::SgrUnderline::DASHED => Ok(Self::Dashed),
            _ => Err(Error::InvalidValue),
        }
    }
}

impl From<ffi::ColorRgb> for RgbColor {
    fn from(value: ffi::ColorRgb) -> Self {
        Self {
            r: value.r,
            g: value.g,
            b: value.b,
        }
    }
}

impl From<RgbColor> for ffi::ColorRgb {
    fn from(value: RgbColor) -> Self {
        Self {
            r: value.r,
            g: value.g,
            b: value.b,
        }
    }
}
