//! Mightty - Terminal emulator built with GPUI and libghostty-vt

pub mod ghostty;
pub mod input;
pub mod widget;

#[cfg(windows)]
pub mod shell;
