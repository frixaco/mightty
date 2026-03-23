# Input Event Pipeline

## Purpose
Transform GPUI window events into bytes sent to the shell.

## Event Flow

```
GPUI Key Event
    ↓
Input Mapper
    ↓
VT Sequence
    ↓
ConPTY Shell Bridge (write)
```

## Key Mappings

### Basic Keys
| Key | Bytes |
|-----|-------|
| Enter | `\r` (0x0D) |
| Backspace | `\x7F` or `\b` |
| Tab | `\t` |
| Escape | `\x1B` |
| Ctrl+C | `\x03` |
| Ctrl+D | `\x04` |
| Ctrl+L | `\x0C` |

### Cursor Keys (Normal Mode)
| Key | Sequence |
|-----|----------|
| Up | `ESC [ A` |
| Down | `ESC [ B` |
| Right | `ESC [ C` |
| Left | `ESC [ D` |
| Home | `ESC [ H` |
| End | `ESC [ F` |

### Cursor Keys (Application Mode)
| Key | Sequence |
|-----|----------|
| Up | `ESC O A` |
| Down | `ESC O B` |
| Right | `ESC O C` |
| Left | `ESC O D` |
| Home | `ESC O H` |
| End | `ESC O F` |

### Function Keys
| Key | Sequence |
|-----|----------|
| F1-F4 | `ESC O P`, `Q`, `R`, `S` |
| F5 | `ESC [ 1 5 ~` |
| F6 | `ESC [ 1 7 ~` |
| F7 | `ESC [ 1 8 ~` |
| F8 | `ESC [ 1 9 ~` |
| F9 | `ESC [ 2 0 ~` |
| F10 | `ESC [ 2 1 ~` |
| F11 | `ESC [ 2 3 ~` |
| F12 | `ESC [ 2 4 ~` |

### Modifiers (CSI u format)
Format: `ESC [ <code> ; <modifier> u`

Modifiers: 2=Shift, 3=Alt, 4=Shift+Alt, 5=Ctrl, etc.

## Interface

```rust
pub struct InputMapper {
    application_mode: bool,
}

impl InputMapper {
    pub fn new() -> Self;
    
    /// Set cursor key mode
    pub fn set_application_mode(&mut self, enabled: bool);
    
    /// Map GPUI key to VT bytes
    pub fn map_key(&self, key: Key, modifiers: Modifiers) -> Vec<u8>;
}

pub struct Key {
    pub code: KeyCode,
    pub text: Option<String>,  // For text input events
}

pub enum KeyCode {
    Char(char),
    Arrow(Arrow),
    Function(u8),
    Home, End, Insert, Delete,
    PageUp, PageDown,
    Escape, Enter, Backspace, Tab,
}
```

## Unicode Input

- Direct UTF-8 encoding for printable characters
- Alt sends `ESC` prefix + character

## Mouse (Optional)

X10 encoding: `ESC [ M <button+32> <x+32> <y+32>`

## State Management

Track terminal mode changes from shell:
- Application cursor keys (DECCKM)
- Bracketed paste mode
- Mouse tracking modes

## Testing

Unit tests for each key mapping:
```rust
#[test]
fn test_arrow_up() {
    let mapper = InputMapper::new();
    assert_eq!(mapper.map_key(Key::Arrow(Up), Modifiers::none()), 
               b"\x1B[A");
}
```
