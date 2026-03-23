//! Input Event Pipeline
//!
//! Transforms GPUI window events into VT sequences for the shell.

/// Input mapper that converts key events to VT sequences
#[derive(Debug, Clone, Copy, Default)]
pub struct InputMapper {
    /// Application cursor key mode (DECCKM)
    application_mode: bool,
}

impl InputMapper {
    /// Create a new input mapper with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set application cursor key mode
    pub fn set_application_mode(&mut self, enabled: bool) {
        self.application_mode = enabled;
    }

    /// Check if application mode is enabled
    pub fn is_application_mode(&self) -> bool {
        self.application_mode
    }

    /// Map a key event to VT sequence bytes
    pub fn map_key(&self, key: &Key) -> Vec<u8> {
        self.map_key_with_modifiers(key, Modifiers::NONE)
    }

    /// Map a key event with modifiers to VT sequence bytes
    pub fn map_key_with_modifiers(&self, key: &Key, modifiers: Modifiers) -> Vec<u8> {
        match &key.code {
            KeyCode::Char(c) => self.map_char(*c, modifiers),
            KeyCode::Arrow(dir) => self.map_arrow(*dir, modifiers),
            KeyCode::Function(n) => self.map_function(*n, modifiers),
            KeyCode::Home => self.map_home(modifiers),
            KeyCode::End => self.map_end(modifiers),
            KeyCode::Insert => self.map_insert(modifiers),
            KeyCode::Delete => self.map_delete(modifiers),
            KeyCode::PageUp => self.map_page_up(modifiers),
            KeyCode::PageDown => self.map_page_down(modifiers),
            KeyCode::Escape => vec![0x1B],
            KeyCode::Enter => vec![0x0D],
            KeyCode::Backspace => vec![0x7F],
            KeyCode::Tab => vec![0x09],
        }
    }

    fn map_char(&self, c: char, modifiers: Modifiers) -> Vec<u8> {
        // Handle control characters
        if modifiers.contains(Modifiers::CONTROL) && c.is_ascii() {
            let lower = c.to_ascii_lowercase();
            // Ctrl+A through Ctrl+Z produce 0x01-0x1A
            if lower >= 'a' && lower <= 'z' {
                return vec![(lower as u8 - b'a' + 1)];
            }
            // Ctrl+@ produces NUL (0x00)
            if c == '@' {
                return vec![0x00];
            }
            // Ctrl+[ produces ESC (0x1B)
            if c == '[' {
                return vec![0x1B];
            }
            // Ctrl+\ produces FS (0x1C)
            if c == '\\' {
                return vec![0x1C];
            }
            // Ctrl+] produces GS (0x1D)
            if c == ']' {
                return vec![0x1D];
            }
            // Ctrl+^ produces RS (0x1E)
            if c == '^' {
                return vec![0x1E];
            }
            // Ctrl+_ produces US (0x1F)
            if c == '_' {
                return vec![0x1F];
            }
        }

        // Handle Alt prefix
        let mut result = Vec::new();
        if modifiers.contains(Modifiers::ALT) {
            result.push(0x1B); // ESC prefix
        }

        // Encode character as UTF-8
        let mut buf = [0u8; 4];
        let encoded = c.encode_utf8(&mut buf);
        result.extend_from_slice(encoded.as_bytes());

        result
    }

    fn map_arrow(&self, dir: Arrow, modifiers: Modifiers) -> Vec<u8> {
        let suffix = match dir {
            Arrow::Up => 'A',
            Arrow::Down => 'B',
            Arrow::Right => 'C',
            Arrow::Left => 'D',
        };

        if modifiers.is_empty() {
            if self.application_mode {
                // ESC O A/B/C/D
                vec![0x1B, b'O', suffix as u8]
            } else {
                // ESC [ A/B/C/D
                vec![0x1B, b'[', suffix as u8]
            }
        } else {
            // With modifiers: ESC [ 1 ; <modifier> A/B/C/D
            let modifier_num = modifier_to_number(modifiers);
            format!("\x1B[1;{}{}", modifier_num, suffix).into_bytes()
        }
    }

    fn map_function(&self, n: u8, modifiers: Modifiers) -> Vec<u8> {
        let modifier_suffix = if modifiers.is_empty() {
            "".to_string()
        } else {
            format!(";{}", modifier_to_number(modifiers))
        };

        match n {
            1..=4 => {
                // F1-F4: ESC O P/Q/R/S
                let suffix = match n {
                    1 => 'P',
                    2 => 'Q',
                    3 => 'R',
                    _ => 'S',
                };
                if modifiers.is_empty() {
                    vec![0x1B, b'O', suffix as u8]
                } else {
                    format!("\x1B[1{}P", modifier_suffix).into_bytes()
                }
            }
            5 => format!("\x1B[15~{}", modifier_suffix).into_bytes(),
            6 => format!("\x1B[17~{}", modifier_suffix).into_bytes(),
            7 => format!("\x1B[18~{}", modifier_suffix).into_bytes(),
            8 => format!("\x1B[19~{}", modifier_suffix).into_bytes(),
            9 => format!("\x1B[20~{}", modifier_suffix).into_bytes(),
            10 => format!("\x1B[21~{}", modifier_suffix).into_bytes(),
            11 => format!("\x1B[23~{}", modifier_suffix).into_bytes(),
            12 => format!("\x1B[24~{}", modifier_suffix).into_bytes(),
            _ => vec![],
        }
    }

    fn map_home(&self, modifiers: Modifiers) -> Vec<u8> {
        if modifiers.is_empty() {
            if self.application_mode {
                vec![0x1B, b'O', b'H']
            } else {
                vec![0x1B, b'[', b'H']
            }
        } else {
            format!("\x1B[1;{}H", modifier_to_number(modifiers)).into_bytes()
        }
    }

    fn map_end(&self, modifiers: Modifiers) -> Vec<u8> {
        if modifiers.is_empty() {
            if self.application_mode {
                vec![0x1B, b'O', b'F']
            } else {
                vec![0x1B, b'[', b'F']
            }
        } else {
            format!("\x1B[1;{}F", modifier_to_number(modifiers)).into_bytes()
        }
    }

    fn map_insert(&self, modifiers: Modifiers) -> Vec<u8> {
        if modifiers.is_empty() {
            vec![0x1B, b'[', b'2', b'~']
        } else {
            format!("\x1B[2;{}~", modifier_to_number(modifiers)).into_bytes()
        }
    }

    fn map_delete(&self, modifiers: Modifiers) -> Vec<u8> {
        if modifiers.is_empty() {
            vec![0x1B, b'[', b'3', b'~']
        } else {
            format!("\x1B[3;{}~", modifier_to_number(modifiers)).into_bytes()
        }
    }

    fn map_page_up(&self, modifiers: Modifiers) -> Vec<u8> {
        if modifiers.is_empty() {
            vec![0x1B, b'[', b'5', b'~']
        } else {
            format!("\x1B[5;{}~", modifier_to_number(modifiers)).into_bytes()
        }
    }

    fn map_page_down(&self, modifiers: Modifiers) -> Vec<u8> {
        if modifiers.is_empty() {
            vec![0x1B, b'[', b'6', b'~']
        } else {
            format!("\x1B[6;{}~", modifier_to_number(modifiers)).into_bytes()
        }
    }
}

/// Convert modifiers to CSI u modifier number
fn modifier_to_number(modifiers: Modifiers) -> u8 {
    let mut result = 1;
    if modifiers.contains(Modifiers::SHIFT) {
        result += 1;
    }
    if modifiers.contains(Modifiers::ALT) {
        result += 2;
    }
    if modifiers.contains(Modifiers::CONTROL) {
        result += 4;
    }
    result
}

/// Key code types
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// Printable character
    Char(char),
    /// Arrow key
    Arrow(Arrow),
    /// Function key (F1-F12)
    Function(u8),
    /// Home key
    Home,
    /// End key
    End,
    /// Insert key
    Insert,
    /// Delete key
    Delete,
    /// Page Up key
    PageUp,
    /// Page Down key
    PageDown,
    /// Escape key
    Escape,
    /// Enter key
    Enter,
    /// Backspace key
    Backspace,
    /// Tab key
    Tab,
}

/// Arrow key directions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arrow {
    /// Up arrow
    Up,
    /// Down arrow
    Down,
    /// Right arrow
    Right,
    /// Left arrow
    Left,
}

/// Key event structure
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    /// The key code
    pub code: KeyCode,
    /// Text representation (for text input events)
    pub text: Option<String>,
}

impl Key {
    /// Create a new key event
    pub fn new(code: KeyCode) -> Self {
        Self { code, text: None }
    }

    /// Create a new key event with text
    pub fn with_text(code: KeyCode, text: impl Into<String>) -> Self {
        Self {
            code,
            text: Some(text.into()),
        }
    }
}

/// Key modifiers (bitflags)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    /// No modifiers
    pub const NONE: Self = Self(0);
    /// Shift key
    pub const SHIFT: Self = Self(1 << 0);
    /// Alt/Option key
    pub const ALT: Self = Self(1 << 1);
    /// Control key
    pub const CONTROL: Self = Self(1 << 2);
    /// Super/Command/Windows key
    pub const SUPER: Self = Self(1 << 3);

    /// Check if no modifiers are set
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Check if specific modifier is present
    pub fn contains(&self, modifier: Self) -> bool {
        (self.0 & modifier.0) != 0
    }

    /// Add a modifier
    pub fn insert(&mut self, modifier: Self) {
        self.0 |= modifier.0;
    }

    /// Remove a modifier
    pub fn remove(&mut self, modifier: Self) {
        self.0 &= !modifier.0;
    }

    /// Create modifiers from individual flags
    pub fn new(shift: bool, alt: bool, ctrl: bool) -> Self {
        let mut m = Self::NONE;
        if shift {
            m.insert(Self::SHIFT);
        }
        if alt {
            m.insert(Self::ALT);
        }
        if ctrl {
            m.insert(Self::CONTROL);
        }
        m
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Modifiers {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAnd for Modifiers {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl std::ops::BitAndAssign for Modifiers {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_keys() {
        let mapper = InputMapper::new();

        assert_eq!(mapper.map_key(&Key::new(KeyCode::Enter)), b"\x0D");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Backspace)), b"\x7F");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Tab)), b"\x09");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Escape)), b"\x1B");
    }

    #[test]
    fn test_arrow_keys_normal_mode() {
        let mapper = InputMapper::new();

        assert_eq!(
            mapper.map_key(&Key::new(KeyCode::Arrow(Arrow::Up))),
            b"\x1B[A"
        );
        assert_eq!(
            mapper.map_key(&Key::new(KeyCode::Arrow(Arrow::Down))),
            b"\x1B[B"
        );
        assert_eq!(
            mapper.map_key(&Key::new(KeyCode::Arrow(Arrow::Right))),
            b"\x1B[C"
        );
        assert_eq!(
            mapper.map_key(&Key::new(KeyCode::Arrow(Arrow::Left))),
            b"\x1B[D"
        );
    }

    #[test]
    fn test_arrow_keys_application_mode() {
        let mut mapper = InputMapper::new();
        mapper.set_application_mode(true);

        assert_eq!(
            mapper.map_key(&Key::new(KeyCode::Arrow(Arrow::Up))),
            b"\x1BOA"
        );
        assert_eq!(
            mapper.map_key(&Key::new(KeyCode::Arrow(Arrow::Down))),
            b"\x1BOB"
        );
        assert_eq!(
            mapper.map_key(&Key::new(KeyCode::Arrow(Arrow::Right))),
            b"\x1BOC"
        );
        assert_eq!(
            mapper.map_key(&Key::new(KeyCode::Arrow(Arrow::Left))),
            b"\x1BOD"
        );
    }

    #[test]
    fn test_home_end_normal_mode() {
        let mapper = InputMapper::new();

        assert_eq!(mapper.map_key(&Key::new(KeyCode::Home)), b"\x1B[H");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::End)), b"\x1B[F");
    }

    #[test]
    fn test_home_end_application_mode() {
        let mut mapper = InputMapper::new();
        mapper.set_application_mode(true);

        assert_eq!(mapper.map_key(&Key::new(KeyCode::Home)), b"\x1BOH");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::End)), b"\x1BOF");
    }

    #[test]
    fn test_function_keys() {
        let mapper = InputMapper::new();

        // F1-F4 use ESC O prefix
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Function(1))), b"\x1BOP");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Function(2))), b"\x1BOQ");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Function(3))), b"\x1BOR");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Function(4))), b"\x1BOS");

        // F5-F12 use ESC [ prefix with tilde
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Function(5))), b"\x1B[15~");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Function(6))), b"\x1B[17~");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Function(7))), b"\x1B[18~");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Function(8))), b"\x1B[19~");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Function(9))), b"\x1B[20~");
        assert_eq!(
            mapper.map_key(&Key::new(KeyCode::Function(10))),
            b"\x1B[21~"
        );
        assert_eq!(
            mapper.map_key(&Key::new(KeyCode::Function(11))),
            b"\x1B[23~"
        );
        assert_eq!(
            mapper.map_key(&Key::new(KeyCode::Function(12))),
            b"\x1B[24~"
        );
    }

    #[test]
    fn test_control_characters() {
        let mapper = InputMapper::new();

        // Ctrl+C
        assert_eq!(
            mapper.map_key_with_modifiers(&Key::new(KeyCode::Char('c')), Modifiers::CONTROL),
            b"\x03"
        );

        // Ctrl+D
        assert_eq!(
            mapper.map_key_with_modifiers(&Key::new(KeyCode::Char('d')), Modifiers::CONTROL),
            b"\x04"
        );

        // Ctrl+L
        assert_eq!(
            mapper.map_key_with_modifiers(&Key::new(KeyCode::Char('l')), Modifiers::CONTROL),
            b"\x0C"
        );
    }

    #[test]
    fn test_alt_prefix() {
        let mapper = InputMapper::new();

        // Alt+a should send ESC + a
        assert_eq!(
            mapper.map_key_with_modifiers(&Key::new(KeyCode::Char('a')), Modifiers::ALT),
            b"\x1Ba"
        );
    }

    #[test]
    fn test_unicode_input() {
        let mapper = InputMapper::new();

        // UTF-8 encoded characters
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Char('h'))), b"h");
        assert_eq!(
            mapper.map_key(&Key::new(KeyCode::Char('é'))),
            "é".as_bytes()
        );
        assert_eq!(
            mapper.map_key(&Key::new(KeyCode::Char('中'))),
            "中".as_bytes()
        );
    }

    #[test]
    fn test_insert_delete() {
        let mapper = InputMapper::new();

        assert_eq!(mapper.map_key(&Key::new(KeyCode::Insert)), b"\x1B[2~");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::Delete)), b"\x1B[3~");
    }

    #[test]
    fn test_page_up_down() {
        let mapper = InputMapper::new();

        assert_eq!(mapper.map_key(&Key::new(KeyCode::PageUp)), b"\x1B[5~");
        assert_eq!(mapper.map_key(&Key::new(KeyCode::PageDown)), b"\x1B[6~");
    }

    #[test]
    fn test_arrow_with_modifiers() {
        let mapper = InputMapper::new();
        let key = Key::new(KeyCode::Arrow(Arrow::Up));

        // Shift+Up
        assert_eq!(
            mapper.map_key_with_modifiers(&key, Modifiers::SHIFT),
            b"\x1B[1;2A"
        );

        // Ctrl+Up
        assert_eq!(
            mapper.map_key_with_modifiers(&key, Modifiers::CONTROL),
            b"\x1B[1;5A"
        );

        // Alt+Up
        assert_eq!(
            mapper.map_key_with_modifiers(&key, Modifiers::ALT),
            b"\x1B[1;3A"
        );

        // Ctrl+Shift+Up
        assert_eq!(
            mapper.map_key_with_modifiers(&key, Modifiers::CONTROL | Modifiers::SHIFT),
            b"\x1B[1;6A"
        );
    }
}
