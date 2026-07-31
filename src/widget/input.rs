use crate::ghostty::{
    Terminal,
    key::{Action, Encoder, Event, Key, Mods},
};

pub(super) fn encode_key_event(
    key_encoder: &mut Encoder,
    key_event: &mut Event,
    terminal: &Terminal,
    action: Action,
    keystroke: &gpui::Keystroke,
) -> Option<Vec<u8>> {
    let ghostty_key = convert_to_ghostty_key(keystroke);
    let ghostty_mods = convert_modifiers(&keystroke.modifiers);
    let printable_text = printable_text(keystroke, action);
    let unshifted_codepoint = unshifted_codepoint(keystroke);
    let consumed_mods = consumed_mods(
        &keystroke.key,
        ghostty_mods,
        printable_text,
        unshifted_codepoint,
    );

    key_event
        .set_action(action)
        .set_key(ghostty_key)
        .set_mods(ghostty_mods)
        .set_consumed_mods(consumed_mods)
        .set_unshifted_codepoint(unshifted_codepoint)
        .set_utf8(printable_text)
        .set_composing(false);

    key_encoder.set_options_from_terminal(terminal);

    let mut response = Vec::with_capacity(64);
    key_encoder.encode_to_vec(key_event, &mut response).ok()?;
    (!response.is_empty()).then_some(response)
}

fn printable_text(keystroke: &gpui::Keystroke, action: Action) -> Option<&str> {
    if action == Action::Release {
        return None;
    }

    keystroke
        .key_char
        .as_deref()
        .filter(|t| !t.is_empty())
        .or_else(|| {
            if keystroke.key == "space" {
                Some(" ")
            } else if keystroke.key.chars().count() == 1 {
                Some(keystroke.key.as_str())
            } else {
                None
            }
        })
}

fn unshifted_codepoint(keystroke: &gpui::Keystroke) -> char {
    if keystroke.key == "space" {
        return ' ';
    }

    let mut chars = keystroke.key.chars();
    let Some(c) = chars.next() else { return '\0' };
    if chars.next().is_some() {
        return '\0';
    }

    match c {
        'A'..='Z' => c.to_ascii_lowercase(),
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '~' => '`',
        _ => c,
    }
}

fn consumed_mods(key: &str, mods: Mods, text: Option<&str>, ucp: char) -> Mods {
    let Some(t) = text else {
        return Mods::empty();
    };
    let mut chars = t.chars();
    let Some(tc) = chars.next() else {
        return Mods::empty();
    };
    if chars.next().is_some() {
        return Mods::empty();
    }

    if (mods.contains(Mods::SHIFT) && tc != ucp) || key_implies_shift(key, ucp) {
        Mods::SHIFT
    } else {
        Mods::empty()
    }
}

fn key_implies_shift(key: &str, ucp: char) -> bool {
    let mut chars = key.chars();
    let Some(kc) = chars.next() else { return false };
    if chars.next().is_some() {
        return false;
    }

    ucp != '\0' && kc != ucp
}

fn convert_to_ghostty_key(keystroke: &gpui::Keystroke) -> Key {
    match keystroke.key.as_str() {
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "home" => Key::Home,
        "end" => Key::End,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "escape" => Key::Escape,
        "enter" => Key::Enter,
        "backspace" => Key::Backspace,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        _ if keystroke.key.len() == 1 => {
            let c = keystroke.key.chars().next().unwrap_or('?');
            match c.to_ascii_lowercase() {
                'a'..='z' => match c {
                    'a' => Key::A,
                    'b' => Key::B,
                    'c' => Key::C,
                    'd' => Key::D,
                    'e' => Key::E,
                    'f' => Key::F,
                    'g' => Key::G,
                    'h' => Key::H,
                    'i' => Key::I,
                    'j' => Key::J,
                    'k' => Key::K,
                    'l' => Key::L,
                    'm' => Key::M,
                    'n' => Key::N,
                    'o' => Key::O,
                    'p' => Key::P,
                    'q' => Key::Q,
                    'r' => Key::R,
                    's' => Key::S,
                    't' => Key::T,
                    'u' => Key::U,
                    'v' => Key::V,
                    'w' => Key::W,
                    'x' => Key::X,
                    'y' => Key::Y,
                    'z' => Key::Z,
                    _ => Key::Unidentified,
                },
                '0' => Key::Digit0,
                '1' => Key::Digit1,
                '2' => Key::Digit2,
                '3' => Key::Digit3,
                '4' => Key::Digit4,
                '5' => Key::Digit5,
                '6' => Key::Digit6,
                '7' => Key::Digit7,
                '8' => Key::Digit8,
                '9' => Key::Digit9,
                '-' => Key::Minus,
                '=' => Key::Equal,
                '[' => Key::BracketLeft,
                ']' => Key::BracketRight,
                ';' => Key::Semicolon,
                '\'' => Key::Quote,
                ',' => Key::Comma,
                '.' => Key::Period,
                '/' => Key::Slash,
                '\\' => Key::Backslash,
                '`' => Key::Backquote,
                _ => Key::Unidentified,
            }
        }
        _ => Key::Unidentified,
    }
}

pub(super) fn convert_modifiers(modifiers: &gpui::Modifiers) -> Mods {
    let mut mods = Mods::empty();
    if modifiers.shift {
        mods |= Mods::SHIFT;
    }
    if modifiers.alt {
        mods |= Mods::ALT;
    }
    if modifiers.control {
        mods |= Mods::CTRL;
    }
    if modifiers.platform {
        mods |= Mods::SUPER;
    }
    mods
}
