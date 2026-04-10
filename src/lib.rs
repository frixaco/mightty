//! Mightty - Terminal emulator built with GPUI and libghostty-vt

pub mod ghostty;
pub mod feedback;
pub mod widget;

#[cfg(windows)]
pub mod shell;

pub mod pane;
pub mod split;
pub mod pane_container;
