//! Windows application lifecycle services.

mod default_terminal;
mod hotkey;
mod instance;
mod quick_terminal;

pub use default_terminal::{
    DefaultTerminalHandoff, DefaultTerminalResponse, DefaultTerminalServer,
    show_default_terminal_window,
};
pub use hotkey::GlobalHotKey;
pub use instance::{ControlServer, discover_control_instances, send_control};
pub use instance::{InstanceClaim, PrimaryInstance, claim_instance, send_activation};
pub use quick_terminal::{
    hide_quick_terminal, quick_terminal_has_focus, quick_terminal_is_visible,
    request_window_attention, show_quick_terminal, toggle_quick_terminal,
};
