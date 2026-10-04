use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use pab_protocol::{DesktopInputEvent, DesktopMouseButton};
use std::sync::Mutex;

// Preserve modifiers and held keys between individual browser events.
struct State {
    engine: Enigo,
    buttons: Vec<Button>,
}
static ENGINE: Mutex<Option<State>> = Mutex::new(None);
pub fn apply_input(event: DesktopInputEvent) -> Result<(), String> {
    super::require_accessibility()?;
    if !super::active_console() {
        return Err("macOS input requires the active console user".into());
    }
    if matches!(event, DesktopInputEvent::SecureAttention) {
        return Err("secure attention is only supported on Windows".into());
    }
    let mut slot = ENGINE.lock().map_err(|_| "input engine unavailable")?;
    if slot.is_none() {
        *slot = Some(State {
            engine: Enigo::new(&Settings {
                open_prompt_to_get_permissions: false,
                ..Default::default()
            })
            .map_err(|e| e.to_string())?,
            buttons: Vec::new(),
        });
    }
    let State { engine, buttons } = slot.as_mut().unwrap();
    match event {
        DesktopInputEvent::MouseMove { x, y } => {
            let monitors = xcap::Monitor::all().map_err(|e| e.to_string())?;
            let monitor = monitors
                .iter()
                .find(|m| m.is_primary().unwrap_or(false))
                .ok_or("primary monitor unavailable")?;
            let width = monitor.width().map_err(|e| e.to_string())?;
            let height = monitor.height().map_err(|e| e.to_string())?;
            if width == 0 || height == 0 {
                return Err("empty display geometry".into());
            }
            // xcap/CGDisplayBounds and CGEvent both use logical display points on macOS.
            let px = i64::from(monitor.x().map_err(|e| e.to_string())?)
                + i64::from(x) * i64::from(width - 1) / 65535;
            let py = i64::from(monitor.y().map_err(|e| e.to_string())?)
                + i64::from(y) * i64::from(height - 1) / 65535;
            engine
                .move_mouse(px as i32, py as i32, Coordinate::Abs)
                .map_err(|e| e.to_string())
        }
        DesktopInputEvent::MouseButton { button, down } => {
            let button = match button {
                DesktopMouseButton::Left => Button::Left,
                DesktopMouseButton::Right => Button::Right,
                DesktopMouseButton::Middle => Button::Middle,
            };
            // Retain attempted presses so cleanup also covers partial API failure.
            if down && !buttons.contains(&button) {
                buttons.push(button);
            }
            engine
                .button(button, direction(down))
                .map_err(|e| e.to_string())?;
            if !down {
                buttons.retain(|b| *b != button);
            }
            Ok(())
        }
        DesktopInputEvent::MouseWheel { delta } => {
            let amount = -(i32::from(delta).signum()
                * i32::from(delta).unsigned_abs().div_ceil(120).min(10) as i32);
            engine
                .scroll(amount, Axis::Vertical)
                .map_err(|e| e.to_string())
        }
        DesktopInputEvent::Key { virtual_key, down } => {
            let key = map_virtual_key(virtual_key)
                .ok_or_else(|| format!("unsupported browser virtual key: {virtual_key}"))?;
            engine.key(key, direction(down)).map_err(|e| e.to_string())
        }
        DesktopInputEvent::SecureAttention => unreachable!(),
    }
}
pub fn release_input() {
    if let Ok(mut slot) = ENGINE.lock()
        && let Some(mut state) = slot.take()
    {
        for button in state.buttons {
            let _ = state.engine.button(button, Direction::Release);
        }
        // Enigo's Drop releases all keys tracked by this engine.
    }
}
fn direction(down: bool) -> Direction {
    if down {
        Direction::Press
    } else {
        Direction::Release
    }
}
fn map_virtual_key(value: u16) -> Option<Key> {
    let key = match value {
        8 => Key::Backspace,
        9 => Key::Tab,
        13 => Key::Return,
        16 | 160 => Key::LShift,
        161 => Key::RShift,
        17 | 162 => Key::LControl,
        163 => Key::RControl,
        18 | 164 => Key::Alt,
        165 => Key::Option,
        20 => Key::CapsLock,
        27 => Key::Escape,
        32 => Key::Space,
        33 => Key::PageUp,
        34 => Key::PageDown,
        35 => Key::End,
        36 => Key::Home,
        37 => Key::LeftArrow,
        38 => Key::UpArrow,
        39 => Key::RightArrow,
        40 => Key::DownArrow,
        46 => Key::Delete,
        48..=57 => Key::Unicode(value as u8 as char),
        65..=90 => Key::Unicode((value as u8 + 32) as char),
        91 | 92 | 93 | 224 => Key::Meta,
        96 => Key::Numpad0,
        97 => Key::Numpad1,
        98 => Key::Numpad2,
        99 => Key::Numpad3,
        100 => Key::Numpad4,
        101 => Key::Numpad5,
        102 => Key::Numpad6,
        103 => Key::Numpad7,
        104 => Key::Numpad8,
        105 => Key::Numpad9,
        106 => Key::Multiply,
        107 => Key::Add,
        109 => Key::Subtract,
        110 => Key::Decimal,
        111 => Key::Divide,
        112 => Key::F1,
        113 => Key::F2,
        114 => Key::F3,
        115 => Key::F4,
        116 => Key::F5,
        117 => Key::F6,
        118 => Key::F7,
        119 => Key::F8,
        120 => Key::F9,
        121 => Key::F10,
        122 => Key::F11,
        123 => Key::F12,
        124 => Key::F13,
        125 => Key::F14,
        126 => Key::F15,
        127 => Key::F16,
        128 => Key::F17,
        129 => Key::F18,
        130 => Key::F19,
        131 => Key::F20,
        186 => Key::Unicode(';'),
        187 => Key::Unicode('='),
        188 => Key::Unicode(','),
        189 => Key::Unicode('-'),
        190 => Key::Unicode('.'),
        191 => Key::Unicode('/'),
        192 => Key::Unicode('`'),
        219 => Key::Unicode('['),
        220 => Key::Unicode('\\'),
        221 => Key::Unicode(']'),
        222 => Key::Unicode('\''),
        _ => return None,
    };
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_mac_modifiers_and_rejects_unknown() {
        assert_eq!(map_virtual_key(91), Some(Key::Meta));
        assert_eq!(map_virtual_key(17), Some(Key::LControl));
        assert_eq!(map_virtual_key(65), Some(Key::Unicode('a')));
        assert_eq!(map_virtual_key(255), None);
    }
}
