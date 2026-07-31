//! Windows application lifecycle services.

mod hotkey;
mod instance;
mod quick_terminal;

pub use hotkey::GlobalHotKey;
pub use instance::{InstanceClaim, PrimaryInstance, claim_instance, send_activation};
pub use quick_terminal::{
    hide_quick_terminal, quick_terminal_has_focus, quick_terminal_is_visible, show_quick_terminal,
    toggle_quick_terminal,
};
