//! Safe, project-owned Rust interface to Ghostty's `libghostty-vt`.
//!
//! The raw C interface is generated from the pinned Ghostty submodule and kept
//! private. Callers use terminal concepts and never handle C pointers.

mod error;
mod search;
mod selection;
mod semantic;
mod terminal;

#[cfg(test)]
mod abi;

pub mod graphics;
pub mod key;
pub mod mouse;
pub mod paste;
pub mod render;
pub mod style;

#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    clippy::all,
    rustdoc::all
)]
#[rustfmt::skip]
mod ffi;

pub use error::{Error, Result};
pub use render::RenderState;
pub use search::{SearchDirection, SearchPoint, SearchProgress, SearchRange};
pub use selection::{SelectionDrag, SelectionGeometry, SelectionPoint, SelectionPress};
pub use semantic::PromptDirection;
pub use terminal::{
    ClipboardContent, ClipboardLocation, ClipboardWrite, ClipboardWriteResult, Scrollbar, Terminal,
    TerminalOptions, ViewportScroll,
};

/// Exact Ghostty source revision compiled into this build.
pub const SOURCE_REVISION: &str = env!("MIGHTTY_GHOSTTY_REVISION");
