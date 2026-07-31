//! Command-palette entries and fuzzy filtering.

use crate::action::{ActionAvailability, ActionCategory, ActionContext, AppAction, SplitDirection};
use crate::settings::ResolvedSettings;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteCommand {
    pub action: AppAction,
    pub title: String,
    pub category: ActionCategory,
    pub binding: Option<String>,
    pub unavailable_reason: Option<&'static str>,
}

pub fn commands(settings: &ResolvedSettings, context: ActionContext) -> Vec<PaletteCommand> {
    let mut commands = AppAction::catalog()
        .into_iter()
        .filter(|action| !matches!(action, AppAction::CommandPalette))
        .map(|action| command_for_action(action, None, settings, context))
        .collect::<Vec<_>>();

    for profile in settings.profiles.values() {
        commands.push(command_for_action(
            AppAction::NewTab {
                profile_id: Some(profile.id.clone()),
            },
            Some(format!("New tab: {}", profile.label)),
            settings,
            context,
        ));
        commands.push(command_for_action(
            AppAction::Split {
                direction: SplitDirection::Right,
                profile_id: Some(profile.id.clone()),
            },
            Some(format!("Split right: {}", profile.label)),
            settings,
            context,
        ));
        commands.push(command_for_action(
            AppAction::Split {
                direction: SplitDirection::Down,
                profile_id: Some(profile.id.clone()),
            },
            Some(format!("Split down: {}", profile.label)),
            settings,
            context,
        ));
    }
    commands
}

pub fn filtered_command_indices(commands: &[PaletteCommand], query: &str) -> Vec<usize> {
    let mut matches = commands
        .iter()
        .enumerate()
        .filter_map(|(index, command)| {
            fuzzy_score(query, &command.title).map(|score| (index, score))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score.cmp(left_score).then_with(|| {
            commands[*left_index]
                .title
                .cmp(&commands[*right_index].title)
        })
    });
    matches.into_iter().map(|(index, _)| index).collect()
}

fn command_for_action(
    action: AppAction,
    title: Option<String>,
    settings: &ResolvedSettings,
    context: ActionContext,
) -> PaletteCommand {
    let descriptor = action.descriptor();
    let unavailable_reason = match action.availability(context) {
        ActionAvailability::Available => None,
        ActionAvailability::Unavailable(reason) => Some(reason),
    };
    let binding = preferred_binding(settings, &action);
    PaletteCommand {
        title: title.unwrap_or_else(|| descriptor.title.to_string()),
        category: descriptor.category,
        binding,
        unavailable_reason,
        action,
    }
}

fn preferred_binding(settings: &ResolvedSettings, action: &AppAction) -> Option<String> {
    let bindings = settings
        .key_bindings
        .iter()
        .filter(|binding| binding.action == *action);
    #[cfg(target_os = "macos")]
    let bindings =
        bindings.map(|binding| (!binding.chord.starts_with("cmd-"), binding.chord.clone()));
    #[cfg(not(target_os = "macos"))]
    let bindings =
        bindings.map(|binding| (binding.chord.starts_with("cmd-"), binding.chord.clone()));
    bindings.min().map(|(_, chord)| chord)
}

fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Some(0);
    }

    let candidate = candidate.to_ascii_lowercase();
    let mut score = 0;
    let mut search_start = 0;
    let mut previous_match = None;
    for query_character in query.chars() {
        let (offset, _) = candidate[search_start..]
            .char_indices()
            .find(|(_, candidate_character)| *candidate_character == query_character)?;
        let index = search_start + offset;
        score += 10;
        if index == 0
            || candidate[..index]
                .chars()
                .next_back()
                .is_some_and(|character| !character.is_alphanumeric())
        {
            score += 8;
        }
        if previous_match.is_some_and(|previous| previous + 1 == index) {
            score += 6;
        }
        score -= index as i32;
        previous_match = Some(index);
        search_start = index + query_character.len_utf8();
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_filter_prefers_compact_word_matches() {
        let commands = vec![
            test_command("Toggle sidebar"),
            test_command("Split down"),
            test_command("New tab"),
        ];

        let matches = filtered_command_indices(&commands, "sd");
        assert_eq!(matches, [1, 0]);
    }

    #[test]
    fn fuzzy_filter_rejects_out_of_order_text() {
        assert_eq!(fuzzy_score("zx", "Toggle zoom"), None);
        assert!(fuzzy_score("tz", "Toggle zoom").is_some());
    }

    fn test_command(title: &str) -> PaletteCommand {
        PaletteCommand {
            action: AppAction::Quit,
            title: title.to_string(),
            category: ActionCategory::Application,
            binding: None,
            unavailable_reason: None,
        }
    }
}
