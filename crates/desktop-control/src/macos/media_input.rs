//! One remote-view generation owns its input state. It never borrows the
//! browser/legacy input engine. Native state and its destructor stay on AppKit.
use super::{input::map_virtual_key, recovery::RecoveryInput};
use crate::macos_display::DisplayGeometry;
use enigo::{Axis, Button, Coordinate, Direction};
use pab_protocol::{DesktopMouseButton, DesktopRotation, DesktopScreen, RemoteDesktopInput};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

struct State {
    engine: RecoveryInput,
    // Keep the native code chosen on key-down. A layout change before key-up
    // must release that same physical key, not translate the virtual key again.
    keys: BTreeMap<u16, u16>,
}
struct Inner {
    stopped: AtomicBool,
    state: Mutex<Option<State>>,
}

pub struct MacMediaInput {
    inner: Arc<Inner>,
    screen: DesktopScreen,
    geometry: DisplayGeometry,
}

fn matches_screen(screen: &DesktopScreen, actual: DisplayGeometry) -> bool {
    screen.id.parse::<u32>().ok() == Some(actual.id)
        && screen.rotation == DesktopRotation::Identity
        && (
            screen.x,
            screen.y,
            screen.desktop_width,
            screen.desktop_height,
        ) == (actual.x, actual.y, actual.width, actual.height)
        && screen.scale_milli == (actual.scale * 1000.0).round() as u32
        && screen.pixel_width == (actual.width as f64 * f64::from(actual.scale)).round() as u32
        && screen.pixel_height == (actual.height as f64 * f64::from(actual.scale)).round() as u32
}

fn point(geometry: DisplayGeometry, x: u16, y: u16) -> Result<(i32, i32), String> {
    let axis = |origin: i32, length: u32, value: u16| {
        if length == 0 {
            return Err("empty media input geometry".to_string());
        }
        i32::try_from(i64::from(origin) + i64::from(value) * i64::from(length - 1) / 65535)
            .map_err(|_| "media input coordinate overflow".to_string())
    };
    Ok((
        axis(geometry.x, geometry.width, x)?,
        axis(geometry.y, geometry.height, y)?,
    ))
}

fn require_session() -> Result<(), String> {
    super::require_accessibility()?;
    if !super::active_console() {
        return Err("media input requires the active console session".into());
    }
    Ok(())
}

impl MacMediaInput {
    /// Call from a worker, not AppKit: display wake/selection may wait up to 2s.
    pub fn new(screen: DesktopScreen) -> Result<Self, String> {
        screen.validate().map_err(str::to_owned)?;
        require_session()?;
        let id = screen.id.parse().map_err(|_| "invalid media display ID")?;
        let (_, geometry) = crate::select_macos_monitor(Some(id))?;
        if !matches_screen(&screen, geometry) {
            return Err("media display geometry changed".into());
        }
        Ok(Self {
            inner: Arc::new(Inner {
                stopped: AtomicBool::new(false),
                state: Mutex::new(None),
            }),
            screen,
            geometry,
        })
    }

    /// Build a cancellable operation for a blocking worker. Dropping the owner
    /// stops queued operations before they can post input; no event is replayed.
    pub fn operation(
        &self,
        event: RemoteDesktopInput,
    ) -> impl FnOnce() -> Result<(), String> + Send + 'static {
        let inner = self.inner.clone();
        let screen = self.screen.clone();
        let expected = self.geometry;
        move || {
            event.validate().map_err(str::to_owned)?;
            if inner.stopped.load(Ordering::Acquire) {
                return Err("media input generation ended".into());
            }
            if !matches!(event, RemoteDesktopInput::ReleaseAll) {
                require_session()?;
                let (_, actual) = crate::select_macos_monitor(Some(expected.id))?;
                if actual != expected || !matches_screen(&screen, actual) {
                    return Err("media display geometry changed; input not replayed".into());
                }
            }
            crate::on_input_thread(move || {
                if inner.stopped.load(Ordering::Acquire) {
                    return Err("media input generation ended".into());
                }
                let mut state = inner
                    .state
                    .lock()
                    .map_err(|_| "media input state unavailable")?;
                if matches!(event, RemoteDesktopInput::ReleaseAll) {
                    state.take(); // RecoveryInput releases its journaled native keys/buttons.
                    return Ok(());
                }
                require_session()?;
                if state.is_none() {
                    *state = Some(State {
                        engine: RecoveryInput::new()?,
                        keys: BTreeMap::new(),
                    });
                }
                let state = state.as_mut().unwrap();
                let direction = |down| {
                    if down {
                        Direction::Press
                    } else {
                        Direction::Release
                    }
                };
                match event {
                    RemoteDesktopInput::Move { x, y } => {
                        let (x, y) = point(expected, x, y)?;
                        state
                            .engine
                            .move_mouse(x, y, Coordinate::Abs)
                            .map_err(|e| e.to_string())
                    }
                    RemoteDesktopInput::Button { x, y, button, down } => {
                        let (x, y) = point(expected, x, y)?;
                        state
                            .engine
                            .move_mouse(x, y, Coordinate::Abs)
                            .map_err(|e| e.to_string())?;
                        state.engine.button(
                            match button {
                                DesktopMouseButton::Left => Button::Left,
                                DesktopMouseButton::Right => Button::Right,
                                DesktopMouseButton::Middle => Button::Middle,
                            },
                            direction(down),
                        )
                    }
                    RemoteDesktopInput::Wheel { delta } => {
                        let amount = -(i32::from(delta).signum()
                            * i32::from(delta).unsigned_abs().div_ceil(120).min(10) as i32);
                        state
                            .engine
                            .scroll(amount, Axis::Vertical)
                            .map_err(|e| e.to_string())
                    }
                    RemoteDesktopInput::Key { virtual_key, down } => {
                        let code = if let Some(code) = state.keys.get(&virtual_key) {
                            *code
                        } else if !down {
                            return Ok(());
                        } else {
                            let key = map_virtual_key(virtual_key)
                                .ok_or("unsupported media virtual key")?;
                            let code =
                                u16::try_from(key).map_err(|_| "unsupported media native key")?;
                            state.keys.insert(virtual_key, code);
                            code
                        };
                        state.engine.raw_key(code, direction(down))?;
                        if !down {
                            state.keys.remove(&virtual_key);
                        }
                        Ok(())
                    }
                    RemoteDesktopInput::ReleaseAll => unreachable!(),
                }
            })
        }
    }
}

impl Drop for MacMediaInput {
    fn drop(&mut self) {
        self.inner.stopped.store(true, Ordering::Release);
        // No AppKit object exists until the first accepted operation. Once
        // stopped, queued operations cannot create one; skip empty dispatches.
        if self.inner.state.lock().is_ok_and(|state| state.is_none()) {
            return;
        }
        let inner = self.inner.clone();
        crate::on_input_thread(move || {
            if let Ok(mut state) = inner.state.lock() {
                state.take();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_display_coordinates_include_negative_origin_and_retina_points() {
        let g = DisplayGeometry {
            id: 7,
            x: -1440,
            y: -200,
            width: 1440,
            height: 900,
            scale: 2.0,
        };
        assert_eq!(point(g, 0, 0).unwrap(), (-1440, -200));
        assert_eq!(point(g, 65535, 65535).unwrap(), (-1, 699));
        assert!(point(DisplayGeometry { width: 0, ..g }, 0, 0).is_err());
        assert!(point(DisplayGeometry { x: i32::MAX, ..g }, 65535, 0).is_err());
        let screen = DesktopScreen {
            id: "7".into(),
            name: "test".into(),
            x: -1440,
            y: -200,
            desktop_width: 1440,
            desktop_height: 900,
            pixel_width: 2880,
            pixel_height: 1800,
            scale_milli: 2000,
            rotation: DesktopRotation::Identity,
            primary: false,
        };
        assert!(matches_screen(&screen, g));
        assert!(!matches_screen(&screen, DisplayGeometry { id: 8, ..g }));
        assert!(!matches_screen(&screen, DisplayGeometry { x: 0, ..g }));
        assert!(!matches_screen(
            &screen,
            DisplayGeometry { scale: 1.0, ..g }
        ));
    }

    #[test]
    fn queued_input_cannot_initialize_or_post_after_its_owner_is_dropped() {
        let geometry = DisplayGeometry {
            id: 1,
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
        };
        let screen = DesktopScreen {
            id: "1".into(),
            name: "test".into(),
            x: 0,
            y: 0,
            desktop_width: 1920,
            desktop_height: 1080,
            pixel_width: 1920,
            pixel_height: 1080,
            scale_milli: 1000,
            rotation: DesktopRotation::Identity,
            primary: true,
        };
        let input = MacMediaInput {
            inner: Arc::new(Inner {
                stopped: AtomicBool::new(false),
                state: Mutex::new(None),
            }),
            screen,
            geometry,
        };
        let queued = input.operation(RemoteDesktopInput::Key {
            virtual_key: 65,
            down: true,
        });
        drop(input);
        assert_eq!(queued().unwrap_err(), "media input generation ended");
    }
}
