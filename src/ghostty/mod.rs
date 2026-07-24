//! Safe, project-owned Rust interface to Ghostty's `libghostty-vt`.
//!
//! The raw C interface is generated from the pinned Ghostty submodule and kept
//! private. Callers use terminal concepts and never handle C pointers.

mod error;
mod terminal;

#[cfg(test)]
mod abi;

pub mod key;
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
pub use terminal::{Terminal, TerminalOptions};

/// Exact Ghostty source revision compiled into this build.
pub const SOURCE_REVISION: &str = env!("MIGHTTY_GHOSTTY_REVISION");
