use serde::{Deserialize, Serialize};

use crate::profile::ProfileId;

/// One application command shared by shortcuts, menus, palettes, and IPC.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppAction {
    NewTab {
        profile_id: Option<ProfileId>,
    },
    Split {
        direction: SplitDirection,
        profile_id: Option<ProfileId>,
    },
    ClosePane,
    FocusPane {
        direction: Direction,
    },
    ResizePane {
        direction: Direction,
        amount: u16,
    },
    TogglePaneZoom,
    Copy,
    Paste,
    Search,
    ToggleSidebar,
    SelectTab {
        index: u8,
    },
    CommandPalette,
    ToggleQuickTerminal,
    Quit,
}

impl AppAction {
    pub fn descriptor(&self) -> ActionDescriptor {
        match self {
            Self::NewTab { .. } => descriptor(
                "new_tab",
                "New tab",
                ActionCategory::Terminal,
                &["ctrl-t", "cmd-t"],
            ),
            Self::Split {
                direction: SplitDirection::Right,
                ..
            } => descriptor(
                "split_right",
                "Split right",
                ActionCategory::Pane,
                &["alt-enter"],
            ),
            Self::Split {
                direction: SplitDirection::Down,
                ..
            } => descriptor(
                "split_down",
                "Split down",
                ActionCategory::Pane,
                &["alt-shift-enter"],
            ),
            Self::ClosePane => descriptor(
                "close_pane",
                "Close pane",
                ActionCategory::Pane,
                &["ctrl-d"],
            ),
            Self::FocusPane { .. } => {
                descriptor("focus_pane", "Focus pane", ActionCategory::Pane, &[])
            }
            Self::ResizePane { .. } => {
                descriptor("resize_pane", "Resize pane", ActionCategory::Pane, &[])
            }
            Self::TogglePaneZoom => descriptor(
                "toggle_pane_zoom",
                "Toggle pane zoom",
                ActionCategory::Pane,
                &[],
            ),
            Self::Copy => descriptor(
                "copy",
                "Copy selection",
                ActionCategory::Terminal,
                &["ctrl-shift-c", "cmd-c"],
            ),
            Self::Paste => descriptor(
                "paste",
                "Paste",
                ActionCategory::Terminal,
                &["ctrl-shift-v", "cmd-v"],
            ),
            Self::Search => descriptor(
                "search",
                "Search terminal",
                ActionCategory::Terminal,
                &["ctrl-shift-f", "cmd-f"],
            ),
            Self::ToggleSidebar => descriptor(
                "toggle_sidebar",
                "Toggle sidebar",
                ActionCategory::Window,
                &["ctrl-b"],
            ),
            Self::SelectTab { .. } => {
                descriptor("select_tab", "Select tab", ActionCategory::Window, &[])
            }
            Self::CommandPalette => descriptor(
                "command_palette",
                "Command palette",
                ActionCategory::Window,
                &["ctrl-shift-p", "cmd-shift-p"],
            ),
            Self::ToggleQuickTerminal => descriptor(
                "toggle_quick_terminal",
                "Toggle quick terminal",
                ActionCategory::Window,
                &[],
            ),
            Self::Quit => descriptor("quit", "Quit", ActionCategory::Application, &["cmd-q"]),
        }
    }

    pub fn availability(&self, context: ActionContext) -> ActionAvailability {
        match self {
            Self::Copy if !context.has_selection => {
                ActionAvailability::Unavailable("No terminal text is selected")
            }
            Self::FocusPane { .. } | Self::ResizePane { .. } if context.pane_count < 2 => {
                ActionAvailability::Unavailable("The tab has only one pane")
            }
            Self::Search if !context.search_available => {
                ActionAvailability::Unavailable("Terminal search is unavailable")
            }
            _ => ActionAvailability::Available,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Right,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionCategory {
    Application,
    Window,
    Pane,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionDescriptor {
    pub id: &'static str,
    pub title: &'static str,
    pub category: ActionCategory,
    pub default_bindings: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActionContext {
    pub has_selection: bool,
    pub pane_count: usize,
    pub search_available: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionAvailability {
    Available,
    Unavailable(&'static str),
}

const fn descriptor(
    id: &'static str,
    title: &'static str,
    category: ActionCategory,
    default_bindings: &'static [&'static str],
) -> ActionDescriptor {
    ActionDescriptor {
        id,
        title,
        category,
        default_bindings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_payload_has_a_stable_serialized_shape() {
        let action = AppAction::Split {
            direction: SplitDirection::Right,
            profile_id: Some(ProfileId::new("powershell").unwrap()),
        };
        let json = serde_json::to_string(&action).unwrap();

        assert_eq!(
            json,
            r#"{"type":"split","direction":"right","profile_id":"powershell"}"#
        );
        assert_eq!(serde_json::from_str::<AppAction>(&json).unwrap(), action);
    }

    #[test]
    fn descriptors_keep_titles_and_default_bindings_together() {
        let descriptor = AppAction::Paste.descriptor();

        assert_eq!(descriptor.id, "paste");
        assert_eq!(descriptor.title, "Paste");
        assert_eq!(descriptor.default_bindings, ["ctrl-shift-v", "cmd-v"]);
    }

    #[test]
    fn availability_explains_context_requirements() {
        let unavailable = AppAction::Copy.availability(ActionContext::default());
        assert_eq!(
            unavailable,
            ActionAvailability::Unavailable("No terminal text is selected")
        );
        assert_eq!(
            AppAction::Copy.availability(ActionContext {
                has_selection: true,
                ..Default::default()
            }),
            ActionAvailability::Available
        );
    }
}
