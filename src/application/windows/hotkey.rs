//! Process-wide quick-terminal hotkey registration.

use std::io;
use std::ptr::null_mut;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
    VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME, VK_INSERT, VK_LEFT, VK_NEXT,
    VK_OEM_3, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
};

use crate::action::{AppAction, normalize_chord};
use crate::application::ActivationRequest;

const QUICK_TERMINAL_HOTKEY_ID: i32 = 0x4d54;

/// Keeps one Windows global hotkey registered on its message thread.
pub struct GlobalHotKey {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl GlobalHotKey {
    /// Register a normalized key chord and forward it as a typed application action.
    pub fn register(chord: &str, sender: flume::Sender<ActivationRequest>) -> io::Result<Self> {
        let spec = HotKeySpec::parse(chord)?;
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("mightty-global-hotkey".to_string())
            .spawn(move || hotkey_thread(spec, sender, ready_tx))?;
        let thread_id = match ready_rx.recv() {
            Ok(Ok(thread_id)) => thread_id,
            Ok(Err(error)) => {
                let _ = thread.join();
                return Err(error);
            }
            Err(_) => {
                let _ = thread.join();
                return Err(io::Error::other(
                    "global hotkey thread stopped during registration",
                ));
            }
        };

        Ok(Self {
            thread_id,
            thread: Some(thread),
        })
    }
}

impl Drop for GlobalHotKey {
    fn drop(&mut self) {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HotKeySpec {
    modifiers: u32,
    virtual_key: u32,
}

impl HotKeySpec {
    fn parse(chord: &str) -> io::Result<Self> {
        let chord = normalize_chord(chord).map_err(invalid_chord)?;
        let mut parts = chord.split('-').peekable();
        let mut modifiers = MOD_NOREPEAT;
        let mut key = None;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                key = Some(part);
                break;
            }
            modifiers |= match part {
                "ctrl" => MOD_CONTROL,
                "alt" => MOD_ALT,
                "shift" => MOD_SHIFT,
                "cmd" => MOD_WIN,
                "fn" => return Err(invalid_chord("the fn modifier is not global")),
                _ => return Err(invalid_chord(format!("unknown modifier '{part}'"))),
            };
        }

        if modifiers == MOD_NOREPEAT {
            return Err(invalid_chord("a global hotkey must contain a modifier key"));
        }
        let virtual_key = virtual_key(key.unwrap_or_default())
            .ok_or_else(|| invalid_chord("the key is not supported as a Windows global hotkey"))?;
        Ok(Self {
            modifiers,
            virtual_key,
        })
    }
}

fn hotkey_thread(
    spec: HotKeySpec,
    sender: flume::Sender<ActivationRequest>,
    ready: mpsc::SyncSender<io::Result<u32>>,
) {
    let thread_id = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    let mut message = MSG::default();
    unsafe {
        PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
    }
    if unsafe {
        RegisterHotKey(
            null_mut(),
            QUICK_TERMINAL_HOTKEY_ID,
            spec.modifiers,
            spec.virtual_key,
        )
    } == 0
    {
        let _ = ready.send(Err(io::Error::last_os_error()));
        return;
    }
    if ready.send(Ok(thread_id)).is_err() {
        unsafe {
            UnregisterHotKey(null_mut(), QUICK_TERMINAL_HOTKEY_ID);
        }
        return;
    }

    loop {
        let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        if message.message == WM_HOTKEY && message.wParam == QUICK_TERMINAL_HOTKEY_ID as usize {
            let _ = sender.send(ActivationRequest::Dispatch {
                action: AppAction::ToggleQuickTerminal,
            });
        }
    }

    unsafe {
        UnregisterHotKey(null_mut(), QUICK_TERMINAL_HOTKEY_ID);
    }
}

fn virtual_key(key: &str) -> Option<u32> {
    if key.len() == 1 {
        let key = key.as_bytes()[0];
        if key.is_ascii_alphanumeric() {
            return Some(key.to_ascii_uppercase().into());
        }
    }
    if let Some(number) = key
        .strip_prefix('f')
        .and_then(|value| value.parse::<u32>().ok())
        && (1..=24).contains(&number)
    {
        return Some(u32::from(VK_F1) + number - 1);
    }

    Some(u32::from(match key {
        "`" => VK_OEM_3,
        "backspace" => VK_BACK,
        "delete" => VK_DELETE,
        "down" => VK_DOWN,
        "end" => VK_END,
        "enter" => VK_RETURN,
        "escape" => VK_ESCAPE,
        "home" => VK_HOME,
        "insert" => VK_INSERT,
        "left" => VK_LEFT,
        "pagedown" => VK_NEXT,
        "pageup" => VK_PRIOR,
        "right" => VK_RIGHT,
        "space" => VK_SPACE,
        "tab" => VK_TAB,
        "up" => VK_UP,
        _ => return None,
    }))
}

fn invalid_chord(message: impl Into<String>) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("invalid quick-terminal hotkey: {}", message.into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_hotkey() {
        assert_eq!(
            HotKeySpec::parse("ctrl-`").unwrap(),
            HotKeySpec {
                modifiers: MOD_CONTROL | MOD_NOREPEAT,
                virtual_key: u32::from(VK_OEM_3),
            }
        );
    }

    #[test]
    fn parses_function_and_character_keys() {
        assert_eq!(
            HotKeySpec::parse("shift-alt-f24").unwrap(),
            HotKeySpec {
                modifiers: MOD_ALT | MOD_SHIFT | MOD_NOREPEAT,
                virtual_key: u32::from(VK_F1) + 23,
            }
        );
        assert_eq!(
            HotKeySpec::parse("cmd-a").unwrap().virtual_key,
            u32::from(b'A')
        );
    }

    #[test]
    fn rejects_unmodified_or_unsupported_keys() {
        assert!(HotKeySpec::parse("a").is_err());
        assert!(HotKeySpec::parse("ctrl-comma").is_err());
        assert!(HotKeySpec::parse("ctrl-fn-a").is_err());
    }
}
