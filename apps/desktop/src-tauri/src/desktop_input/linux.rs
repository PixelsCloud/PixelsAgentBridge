use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use pab_protocol::{DesktopInputEvent, DesktopMouseButton};
use xcap::Monitor;

pub fn apply(event: DesktopInputEvent) -> Result<(), String> {
    if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
        || std::env::var_os("DISPLAY").is_none()
    {
        return Err("Linux desktop input currently requires an X11 session".to_owned());
    }
    let mut enigo = Enigo::new(&Settings::default()).map_err(|error| error.to_string())?;
    match event {
        DesktopInputEvent::MouseMove { x, y } => {
            let monitors = Monitor::all().map_err(|error| error.to_string())?;
            let monitor = monitors
                .iter()
                .find(|monitor| monitor.is_primary().unwrap_or(false))
                .or_else(|| monitors.first())
                .ok_or_else(|| "no interactive monitor is available".to_owned())?;
            let width = monitor.width().map_err(|error| error.to_string())?;
            let height = monitor.height().map_err(|error| error.to_string())?;
            if width == 0 || height == 0 {
                return Err("primary display is unavailable".to_owned());
            }
            let pixel_x = i64::from(monitor.x().map_err(|error| error.to_string())?)
                + i64::from(x) * i64::from(width - 1) / 65535;
            let pixel_y = i64::from(monitor.y().map_err(|error| error.to_string())?)
                + i64::from(y) * i64::from(height - 1) / 65535;
            enigo
                .move_mouse(pixel_x as i32, pixel_y as i32, Coordinate::Abs)
                .map_err(|error| error.to_string())
        }
        DesktopInputEvent::MouseButton { button, down } => {
            let button = match button {
                DesktopMouseButton::Left => Button::Left,
                DesktopMouseButton::Right => Button::Right,
                DesktopMouseButton::Middle => Button::Middle,
            };
            enigo
                .button(button, direction(down))
                .map_err(|error| error.to_string())
        }
        DesktopInputEvent::MouseWheel { delta } => {
            if delta == 0 {
                return Ok(());
            }
            let amount = i32::from(delta).signum()
                * (i32::from(delta).unsigned_abs().div_ceil(120).min(10) as i32);
            enigo
                .scroll(amount, Axis::Vertical)
                .map_err(|error| error.to_string())
        }
        DesktopInputEvent::Key { virtual_key, down } => {
            let key = map_virtual_key(virtual_key)
                .ok_or_else(|| format!("unsupported browser virtual key: {virtual_key}"))?;
            enigo
                .key(key, direction(down))
                .map_err(|error| error.to_string())
        }
        DesktopInputEvent::SecureAttention => {
            Err("secure attention is only supported on Windows".to_owned())
        }
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
        19 => Key::Pause,
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
        45 => Key::Insert,
        46 => Key::Delete,
        48..=57 => Key::Unicode(value as u8 as char),
        65..=90 => Key::Unicode((value as u8 + 32) as char),
        91 | 92 => Key::Meta,
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
        124..=135 => Key::Other(0xffbe + u32::from(value - 112)),
        144 => Key::Numlock,
        145 => Key::ScrollLock,
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
