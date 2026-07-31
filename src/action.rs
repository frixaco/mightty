use serde::{Deserialize, Serialize};

pub use crate::ghostty::PromptDirection;
use crate::profile::ProfileId;
use crate::workspace::WorkspaceId;

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
    SplitFromCurrentDirectory {
        direction: SplitDirection,
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
    JumpToPrompt {
        direction: PromptDirection,
    },
    SelectCommandOutput,
    CopyCommandOutput,
    ToggleSidebar,
    SelectTab {
        index: u8,
    },
    MoveTab {
        direction: Direction,
    },
    CommandPalette,
    SaveWorkspace {
        workspace_id: WorkspaceId,
    },
    RestoreWorkspace {
        workspace_id: WorkspaceId,
    },
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
            Self::SplitFromCurrentDirectory {
                direction: SplitDirection::Right,
            } => descriptor(
                "split_right_from_current_directory",
                "Split right in current directory",
                ActionCategory::Pane,
                &[],
            ),
            Self::SplitFromCurrentDirectory {
                direction: SplitDirection::Down,
            } => descriptor(
                "split_down_from_current_directory",
                "Split down in current directory",
                ActionCategory::Pane,
                &[],
            ),
            Self::ClosePane => descriptor(
                "close_pane",
                "Close pane",
                ActionCategory::Pane,
                &["ctrl-d"],
            ),
            Self::FocusPane {
                direction: Direction::Left,
            } => descriptor("focus_left", "Focus pane left", ActionCategory::Pane, &[]),
            Self::FocusPane {
                direction: Direction::Right,
            } => descriptor("focus_right", "Focus pane right", ActionCategory::Pane, &[]),
            Self::FocusPane {
                direction: Direction::Up,
            } => descriptor("focus_up", "Focus pane up", ActionCategory::Pane, &[]),
            Self::FocusPane {
                direction: Direction::Down,
            } => descriptor("focus_down", "Focus pane down", ActionCategory::Pane, &[]),
            Self::ResizePane {
                direction: Direction::Left,
                ..
            } => descriptor("resize_left", "Resize pane left", ActionCategory::Pane, &[]),
            Self::ResizePane {
                direction: Direction::Right,
                ..
            } => descriptor(
                "resize_right",
                "Resize pane right",
                ActionCategory::Pane,
                &[],
            ),
            Self::ResizePane {
                direction: Direction::Up,
                ..
            } => descriptor("resize_up", "Resize pane up", ActionCategory::Pane, &[]),
            Self::ResizePane {
                direction: Direction::Down,
                ..
            } => descriptor("resize_down", "Resize pane down", ActionCategory::Pane, &[]),
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
            Self::JumpToPrompt {
                direction: PromptDirection::Previous,
            } => descriptor(
                "jump_to_previous_prompt",
                "Jump to previous prompt",
                ActionCategory::Terminal,
                &[],
            ),
            Self::JumpToPrompt {
                direction: PromptDirection::Next,
            } => descriptor(
                "jump_to_next_prompt",
                "Jump to next prompt",
                ActionCategory::Terminal,
                &[],
            ),
            Self::SelectCommandOutput => descriptor(
                "select_command_output",
                "Select preceding command output",
                ActionCategory::Terminal,
                &[],
            ),
            Self::CopyCommandOutput => descriptor(
                "copy_command_output",
                "Copy preceding command output",
                ActionCategory::Terminal,
                &[],
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
            Self::MoveTab {
                direction: Direction::Up,
            } => descriptor("move_tab_up", "Move tab up", ActionCategory::Window, &[]),
            Self::MoveTab {
                direction: Direction::Down,
            } => descriptor(
                "move_tab_down",
                "Move tab down",
                ActionCategory::Window,
                &[],
            ),
            Self::MoveTab { .. } => descriptor("move_tab", "Move tab", ActionCategory::Window, &[]),
            Self::CommandPalette => descriptor(
                "command_palette",
                "Command palette",
                ActionCategory::Window,
                &["ctrl-shift-p", "cmd-shift-p"],
            ),
            Self::SaveWorkspace { .. } => descriptor(
                "save_workspace",
                "Save workspace",
                ActionCategory::Application,
                &[],
            ),
            Self::RestoreWorkspace { .. } => descriptor(
                "restore_workspace",
                "Restore workspace",
                ActionCategory::Application,
                &[],
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
            Self::FocusPane { .. } | Self::ResizePane { .. } | Self::TogglePaneZoom
                if context.pane_count < 2 =>
            {
                ActionAvailability::Unavailable("The tab has only one pane")
            }
            Self::FocusPane { .. } | Self::ResizePane { .. } | Self::TogglePaneZoom
                if !context.pane_management_available =>
            {
                ActionAvailability::Unavailable("Pane management is unavailable")
            }
            Self::Search if !context.search_available => {
                ActionAvailability::Unavailable("Terminal search is unavailable")
            }
            Self::JumpToPrompt { .. } | Self::SelectCommandOutput | Self::CopyCommandOutput
                if !context.semantic_commands_available =>
            {
                ActionAvailability::Unavailable("The active shell has no semantic prompt markers")
            }
            Self::ToggleQuickTerminal if !context.quick_terminal_available => {
                ActionAvailability::Unavailable("Quick terminal is unavailable")
            }
            Self::MoveTab { .. } if context.tab_count < 2 => {
                ActionAvailability::Unavailable("The window has only one tab")
            }
            Self::SplitFromCurrentDirectory { .. } if !context.has_local_working_directory => {
                ActionAvailability::Unavailable("The active shell has no local working directory")
            }
            _ => ActionAvailability::Available,
        }
    }

    /// Static actions used by the palette and default key-binding table.
    pub fn catalog() -> Vec<Self> {
        let mut actions = vec![
            Self::NewTab { profile_id: None },
            Self::Split {
                direction: SplitDirection::Right,
                profile_id: None,
            },
            Self::Split {
                direction: SplitDirection::Down,
                profile_id: None,
            },
            Self::SplitFromCurrentDirectory {
                direction: SplitDirection::Right,
            },
            Self::SplitFromCurrentDirectory {
                direction: SplitDirection::Down,
            },
            Self::ClosePane,
        ];
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            actions.push(Self::FocusPane { direction });
            actions.push(Self::ResizePane {
                direction,
                amount: 5,
            });
        }
        actions.extend([
            Self::TogglePaneZoom,
            Self::Copy,
            Self::Paste,
            Self::Search,
            Self::JumpToPrompt {
                direction: PromptDirection::Previous,
            },
            Self::JumpToPrompt {
                direction: PromptDirection::Next,
            },
            Self::SelectCommandOutput,
            Self::CopyCommandOutput,
            Self::ToggleSidebar,
            Self::MoveTab {
                direction: Direction::Up,
            },
            Self::MoveTab {
                direction: Direction::Down,
            },
            Self::CommandPalette,
            Self::SaveWorkspace {
                workspace_id: WorkspaceId::default_workspace(),
            },
            Self::ToggleQuickTerminal,
            Self::Quit,
        ]);
        actions
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

impl ActionCategory {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Application => "Application",
            Self::Window => "Window",
            Self::Pane => "Pane",
            Self::Terminal => "Terminal",
        }
    }
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
    pub tab_count: usize,
    pub has_local_working_directory: bool,
    pub semantic_commands_available: bool,
    pub pane_management_available: bool,
    pub search_available: bool,
    pub quick_terminal_available: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionAvailability {
    Available,
    Unavailable(&'static str),
}

/// One normalized key chord mapped to a domain action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionBinding {
    pub chord: String,
    pub action: AppAction,
}

/// Thin GPUI adapter for the domain action model.
#[derive(Clone, gpui::Action, PartialEq, Eq, Deserialize)]
#[action(namespace = mightty, no_json)]
pub struct DispatchAppAction {
    pub action: AppAction,
}

pub fn default_action_bindings() -> Vec<ActionBinding> {
    let mut bindings = Vec::new();
    for action in AppAction::catalog() {
        for chord in action.descriptor().default_bindings {
            bindings.push(ActionBinding {
                chord: (*chord).to_string(),
                action: action.clone(),
            });
        }
    }
    for index in 0..9 {
        bindings.push(ActionBinding {
            chord: format!("ctrl-{}", index + 1),
            action: AppAction::SelectTab { index },
        });
    }
    bindings
}

pub fn normalize_chord(chord: &str) -> Result<String, String> {
    let keystroke = gpui::Keystroke::parse(chord.trim()).map_err(|error| error.to_string())?;
    Ok(chord_for_keystroke(&keystroke))
}

pub fn chord_for_keystroke(keystroke: &gpui::Keystroke) -> String {
    let mut parts = Vec::new();
    if keystroke.modifiers.control {
        parts.push("ctrl".to_string());
    }
    if keystroke.modifiers.alt {
        parts.push("alt".to_string());
    }
    if keystroke.modifiers.shift {
        parts.push("shift".to_string());
    }
    if keystroke.modifiers.platform {
        parts.push("cmd".to_string());
    }
    if keystroke.modifiers.function {
        parts.push("fn".to_string());
    }
    parts.push(keystroke.key.to_ascii_lowercase());
    parts.join("-")
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

        let action = AppAction::JumpToPrompt {
            direction: PromptDirection::Previous,
        };
        assert_eq!(
            serde_json::to_string(&action).unwrap(),
            r#"{"type":"jump_to_prompt","direction":"previous"}"#
        );
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

    #[test]
    fn default_bindings_come_from_action_descriptors() {
        let bindings = default_action_bindings();
        assert!(bindings.contains(&ActionBinding {
            chord: "ctrl-t".to_string(),
            action: AppAction::NewTab { profile_id: None },
        }));
        assert!(bindings.contains(&ActionBinding {
            chord: "ctrl-1".to_string(),
            action: AppAction::SelectTab { index: 0 },
        }));
    }

    #[test]
    fn chord_normalization_matches_runtime_keystrokes() {
        let chord = normalize_chord("SHIFT-CTRL-P").unwrap();
        let keystroke = gpui::Keystroke::parse("ctrl-shift-p").unwrap();

        assert_eq!(chord, "ctrl-shift-p");
        assert_eq!(chord_for_keystroke(&keystroke), chord);
    }

    #[test]
    fn workspace_actions_keep_a_stable_serialized_id() {
        let action = AppAction::RestoreWorkspace {
            workspace_id: WorkspaceId::new("project-a").unwrap(),
        };
        let json = serde_json::to_string(&action).unwrap();

        assert_eq!(
            json,
            r#"{"type":"restore_workspace","workspace_id":"project-a"}"#
        );
        assert_eq!(serde_json::from_str::<AppAction>(&json).unwrap(), action);
    }
}
