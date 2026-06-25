//! Project-local facade over the published libghostty-vt crate.

pub use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    error::{Error, Result},
};

pub mod key {
    pub use libghostty_vt::key::{Action, Encoder, Event, Key, Mods};
}

pub mod render {
    pub use libghostty_vt::render::{CellIterator, RowIterator};
    pub use libghostty_vt::screen::CellWide as CellWidth;
}

pub mod style {
    pub use libghostty_vt::style::{RgbColor, Underline};
}
