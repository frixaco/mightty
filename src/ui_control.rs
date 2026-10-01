//! Inject through GPUI's normal focus, shortcut, text and hit-testing paths.
use crate::control::{self, ControlError, KeyEvent, Request};
use gpui::{App, Keystroke, MouseButton, PlatformInput, Window, point, px};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
static ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Step {
    Text {
        text: String,
    },
    Key {
        key: String,
        #[serde(default)]
        modifiers: Vec<String>,
        #[serde(default)]
        event: KeyEvent,
    },
    Pointer {
        event: String,
        x: f32,
        y: f32,
        #[serde(default)]
        button: Option<String>,
        #[serde(default)]
        modifiers: Vec<String>,
        #[serde(default)]
        delta_x: f32,
        #[serde(default)]
        delta_y: f32,
    },
}
fn invalid(message: impl Into<String>) -> ControlError {
    ControlError::new("invalid_argument", message)
}
fn button(value: &str) -> Result<MouseButton, ControlError> {
    match value {
        "left" => Ok(MouseButton::Left),
        "right" => Ok(MouseButton::Right),
        "middle" => Ok(MouseButton::Middle),
        _ => Err(invalid("button must be left, right or middle")),
    }
}
fn key_id(key: &Keystroke) -> String {
    format!("{:?}:{}", key.modifiers, key.key)
}
fn transition(
    held: &mut BTreeMap<String, Keystroke>,
    stroke: Keystroke,
    event: KeyEvent,
) -> Result<(), ControlError> {
    let id = key_id(&stroke);
    match event {
        KeyEvent::Tap if held.contains_key(&id) => Err(invalid("tap of held key")),
        KeyEvent::Press => {
            if held.contains_key(&id) {
                return Err(invalid("duplicate key press"));
            }
            held.insert(id, stroke);
            Ok(())
        }
        KeyEvent::Repeat if !held.contains_key(&id) => Err(invalid("repeat of unheld key")),
        KeyEvent::Release if held.remove(&id).is_none() => Err(invalid("release of unheld key")),
        _ => Ok(()),
    }
}
pub struct Sequence {
    steps: std::collections::VecDeque<Step>,
    keys: BTreeMap<String, Keystroke>,
    pressed: Option<MouseButton>,
    position: gpui::Point<gpui::Pixels>,
    completed: usize,
    focus_before: String,
    pub receiving_targets: Vec<Value>,
    focus_after: String,
}
impl Drop for Sequence {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}
pub fn prepare(
    request: &Request,
    window: &mut Window,
    cx: &mut App,
) -> Result<Sequence, ControlError> {
    let values = match request.op.as_str() {
        "ui.input" => request
            .args
            .get("steps")
            .cloned()
            .ok_or_else(|| invalid("steps required"))?,
        op => {
            let mut value = serde_json::to_value(&request.args).unwrap();
            value["type"] = json!(op.strip_prefix("ui.").unwrap());
            json!([value])
        }
    };
    let steps: Vec<Step> = serde_json::from_value(values).map_err(|e| invalid(e.to_string()))?;
    if steps.is_empty() || steps.len() > 1024 {
        return Err(invalid("input requires 1..1024 steps"));
    }
    let mut held = BTreeMap::new();
    let mut pressed = None;
    let mut bytes = 0usize;
    for step in &steps {
        match step {
            Step::Text { text } => {
                bytes += text.len();
                if bytes > control::MAX_INPUT_BYTES {
                    return Err(invalid("text byte limit exceeded"));
                }
                if !crate::ghostty::paste::is_safe(text.as_bytes()) {
                    return Err(ControlError::new(
                        "paste_confirmation_required",
                        "UI text requires paste confirmation",
                    ));
                }
            }
            Step::Key {
                key,
                modifiers,
                event,
            } => transition(&mut held, control::keystroke(key, modifiers)?, *event)?,
            Step::Pointer {
                event,
                x,
                y,
                button: which,
                modifiers,
                delta_x,
                delta_y,
            } => {
                if ![x, y, delta_x, delta_y].iter().all(|v| v.is_finite())
                    || *x < 0.
                    || *y < 0.
                    || *x > f32::from(window.viewport_size().width)
                    || *y > f32::from(window.viewport_size().height)
                    || delta_x.abs() > 16384.
                    || delta_y.abs() > 16384.
                {
                    return Err(invalid("pointer coordinates or delta out of range"));
                }
                control::keystroke("space", modifiers)?;
                let which = which.as_deref().map(button).transpose()?;
                match event.as_str() {
                    "press" if pressed.is_none() && which.is_some() => pressed = which,
                    "release" if pressed == which && which.is_some() => pressed = None,
                    "move" | "wheel" if which.is_none() => {}
                    _ => return Err(invalid("invalid pointer event or unbalanced buttons")),
                }
            }
        }
    }
    if request.op == "ui.input" && (!held.is_empty() || pressed.is_some()) {
        return Err(invalid(
            "input sequence must release all held keys and buttons",
        ));
    }
    if request.op == "ui.key"
        && steps.iter().any(|step| {
            matches!(
                step,
                Step::Key {
                    event: KeyEvent::Press | KeyEvent::Repeat | KeyEvent::Release,
                    ..
                }
            )
        })
    {
        return Err(invalid("held keys require a balanced ui input sequence"));
    }
    if request.op == "ui.pointer" && pressed.is_some() {
        return Err(invalid(
            "button presses require a balanced ui input sequence",
        ));
    }
    if ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(ControlError::new(
            "busy",
            "another UI input sequence is active",
        ));
    }
    Ok(Sequence {
        steps: steps.into(),
        keys: BTreeMap::new(),
        pressed: None,
        position: point(px(0.), px(0.)),
        completed: 0,
        receiving_targets: Vec::new(),
        focus_before: format!("{:?}", window.focused(cx)),
        focus_after: String::new(),
    })
}
impl Sequence {
    pub fn next(&mut self, window: &mut Window, cx: &mut App) -> Result<bool, ControlError> {
        let Some(step) = self.steps.pop_front() else {
            return Ok(true);
        };
        let mut keys = std::mem::take(&mut self.keys);
        let mut pressed = self.pressed;
        let mut position = self.position;
        let completed = self.completed;
        if completed > 0 {
            window.refresh();
            window.draw(cx).clear();
        }
        let result = match step {
            Step::Text { text } => {
                if window.commit_text(&text, cx) {
                    Ok(())
                } else {
                    Err(ControlError::new(
                        "no_input_target",
                        "focused component has no text input handler",
                    ))
                }
            }
            Step::Key {
                key,
                modifiers,
                event,
            } => {
                let stroke = control::keystroke(&key, &modifiers)?.with_simulated_ime();
                transition(&mut keys, stroke.clone(), event)?;
                if matches!(event, KeyEvent::Repeat) {
                    let result = window.dispatch_event(
                        PlatformInput::KeyDown(gpui::KeyDownEvent {
                            keystroke: stroke.clone(),
                            is_held: true,
                        }),
                        cx,
                    );
                    if result.propagate
                        && let Some(text) = &stroke.key_char
                    {
                        window.commit_text(text, cx);
                    }
                } else if matches!(event, KeyEvent::Tap | KeyEvent::Press) {
                    window.dispatch_keystroke(stroke.clone(), cx);
                }
                if matches!(event, KeyEvent::Tap | KeyEvent::Release) {
                    window.dispatch_event(
                        PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke: stroke }),
                        cx,
                    );
                }
                Ok(())
            }
            Step::Pointer {
                event,
                x,
                y,
                button: which,
                modifiers,
                delta_x,
                delta_y,
            } => {
                position = point(px(x), px(y));
                let modifiers = control::keystroke("space", &modifiers)?.modifiers;
                let input = match event.as_str() {
                    "press" => {
                        let button = button(which.as_deref().unwrap())?;
                        pressed = Some(button);
                        PlatformInput::MouseDown(gpui::MouseDownEvent {
                            button,
                            position,
                            modifiers,
                            click_count: 1,
                            first_mouse: false,
                        })
                    }
                    "release" => {
                        let button = button(which.as_deref().unwrap())?;
                        pressed = None;
                        PlatformInput::MouseUp(gpui::MouseUpEvent {
                            button,
                            position,
                            modifiers,
                            click_count: 1,
                        })
                    }
                    "wheel" => PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                        position,
                        delta: gpui::ScrollDelta::Pixels(point(px(delta_x), px(delta_y))),
                        modifiers,
                        touch_phase: gpui::TouchPhase::Moved,
                    }),
                    _ => PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position,
                        pressed_button: pressed,
                        modifiers,
                    }),
                };
                window.dispatch_event(input, cx);
                Ok(())
            }
        };
        if let Err(mut error) = result {
            for (_, keystroke) in keys {
                window.dispatch_event(PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }), cx);
            }
            if let Some(button) = pressed {
                window.dispatch_event(
                    PlatformInput::MouseUp(gpui::MouseUpEvent {
                        button,
                        position,
                        ..Default::default()
                    }),
                    cx,
                );
            }
            error.effect = if completed == 0 { "none" } else { "partial" };
            error.details = Box::new(json!({"completed_steps":completed}));
            return Err(error);
        }
        self.keys = keys;
        self.pressed = pressed;
        self.position = position;
        self.completed += 1;
        self.focus_after = format!("{:?}", window.focused(cx));
        Ok(self.steps.is_empty())
    }
    pub fn result(&self) -> Value {
        json!({"completed_steps":self.completed,"dispatch_finished":true,"focus_before":self.focus_before,"focus_after":self.focus_after,"pty_acknowledged":false,"receiving_targets":self.receiving_targets})
    }
    pub fn cancel(&mut self, window: &mut Window, cx: &mut App) -> ControlError {
        for (_, keystroke) in std::mem::take(&mut self.keys) {
            window.dispatch_event(PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }), cx);
        }
        if let Some(button) = self.pressed.take() {
            window.dispatch_event(
                PlatformInput::MouseUp(gpui::MouseUpEvent {
                    button,
                    position: self.position,
                    ..Default::default()
                }),
                cx,
            );
        }
        let mut error = ControlError::new("cancelled", "UI sequence cancelled");
        error.effect = if self.completed == 0 {
            "none"
        } else {
            "partial"
        };
        error.details = Box::new(json!({"completed_steps":self.completed}));
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_key_transitions_before_dispatch() {
        let mut held = BTreeMap::new();
        let key = control::keystroke("a", &[]).unwrap();
        assert!(transition(&mut held, key.clone(), KeyEvent::Release).is_err());
        transition(&mut held, key.clone(), KeyEvent::Press).unwrap();
        assert!(transition(&mut held, key.clone(), KeyEvent::Press).is_err());
        transition(&mut held, key, KeyEvent::Release).unwrap();
        assert!(held.is_empty());
    }
}
