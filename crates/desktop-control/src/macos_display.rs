//! xcap 0.8.2 preserves CGGetActiveDisplayList order on macOS. Apple defines
//! its first entry as the main drawable display, including mirrored displays.
//! Do not combine that snapshot with a later CGDisplayIsMain query: display
//! reconfiguration can make those separate observations disagree.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayGeometry {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

impl DisplayGeometry {
    fn usable(self) -> bool {
        self.id != 0
            && self.width > 0
            && self.height > 0
            && self.scale.is_finite()
            && (1.0..=8.0).contains(&self.scale)
    }
}

// Retry only enumeration/selection, never capture or an input side effect.
// Selection retries are immediate; the separate wake stage waits on the caller's
// worker thread before input is dispatched to the AppKit main thread.
fn select<T>(
    requested: Option<u32>,
    mut enumerate: impl FnMut() -> Result<Vec<(T, Option<DisplayGeometry>)>, String>,
) -> Result<(T, DisplayGeometry), String> {
    let mut error = "selected monitor is unavailable".to_owned();
    for _ in 0..3 {
        let displays = match enumerate() {
            Ok(displays) => displays,
            Err(reason) => {
                error = reason;
                continue;
            }
        };
        let selected = match requested {
            Some(id) => displays
                .into_iter()
                .find(|(_, geometry)| geometry.is_some_and(|g| g.id == id)),
            None => displays.into_iter().next(),
        };
        if let Some((display, Some(geometry))) = selected
            && geometry.usable()
        {
            return Ok((display, geometry));
        }
        // Never silently use another monitor when the requested/main display
        // disappears or has invalid geometry, even if other entries are valid.
        error = "selected monitor is unavailable".to_owned();
    }
    Err(error)
}

#[cfg(target_os = "macos")]
pub fn select_monitor(requested: Option<u32>) -> Result<(xcap::Monitor, DisplayGeometry), String> {
    let mut initial = Some(active_monitors()?);
    select(requested, || {
        initial
            .take()
            .map(Ok)
            .unwrap_or_else(|| xcap::Monitor::all().map_err(|e| e.to_string()))
            .map(|monitors| {
                monitors
                    .into_iter()
                    .map(|monitor| {
                        let geometry = (|| {
                            Some(DisplayGeometry {
                                id: monitor.id().ok()?,
                                x: monitor.x().ok()?,
                                y: monitor.y().ok()?,
                                width: monitor.width().ok()?,
                                height: monitor.height().ok()?,
                                scale: monitor.scale_factor().ok()?,
                            })
                        })();
                        (monitor, geometry)
                    })
                    .collect()
            })
    })
}

// Only read-side preparation is retried; no input is injected to wake a screen.
fn wake_and_enumerate<T>(
    mut enumerate: impl FnMut() -> Result<Vec<T>, String>,
    wake: impl FnOnce() -> Result<(), String>,
    mut pause: impl FnMut(),
) -> Result<Vec<T>, String> {
    let first = enumerate()?;
    if !first.is_empty() {
        return Ok(first);
    }
    wake()?;
    // Active displays return before WindowServer finishes the wake fade. An
    // immediate capture was observed to return a black frame on real hardware.
    // Require 500ms of nonempty observations after waking (awake requests have
    // no delay), still within the two-second overall observation budget.
    let mut ready_samples = 0;
    for _ in 0..40 {
        pause();
        let displays = enumerate()?;
        if displays.is_empty() {
            ready_samples = 0;
        } else {
            ready_samples += 1;
        }
        if ready_samples >= 10 {
            return Ok(displays);
        }
    }
    Err("selected monitor is unavailable".into())
}

#[cfg(target_os = "macos")]
/// Call on the helper worker, never the AppKit main thread: wake may wait 2s.
pub fn active_monitors() -> Result<Vec<xcap::Monitor>, String> {
    use objc2_core_foundation::CFString;
    use objc2_io_kit::{
        IOPMAssertionDeclareUserActivity, IOPMAssertionRelease, IOPMUserActiveType,
    };
    use std::sync::Mutex;
    // Serialize the short wake/observation interval; do not keep a permanent
    // power assertion or change the user's Energy Saver configuration.
    static WAKE: Mutex<()> = Mutex::new(());
    let _wake = WAKE.lock().map_err(|_| "display wake unavailable")?;
    struct Assertion(u32);
    impl Drop for Assertion {
        fn drop(&mut self) {
            IOPMAssertionRelease(self.0);
        }
    }
    let mut assertion = None;
    wake_and_enumerate(
        || {
            if !crate::active_console() {
                return Err("interactive desktop changed".into());
            }
            xcap::Monitor::all().map_err(|e| e.to_string())
        },
        || {
            let name = CFString::from_str("Pixels Agent Bridge desktop request");
            let mut id = 0;
            // SAFETY: name remains alive and id is a valid output pointer.
            let result = unsafe {
                IOPMAssertionDeclareUserActivity(Some(&name), IOPMUserActiveType::Local, &mut id)
            };
            if result != 0 {
                return Err("display wake request failed".into());
            }
            assertion = Some(Assertion(id));
            Ok(())
        },
        || std::thread::sleep(std::time::Duration::from_millis(50)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleeping_display_is_woken_once_without_replaying_input() {
        let mut calls = 0;
        let mut wakes = 0;
        let mut pauses = 0;
        let result = wake_and_enumerate(
            || {
                calls += 1;
                Ok(if calls < 4 { vec![] } else { vec![2] })
            },
            || {
                wakes += 1;
                Ok(())
            },
            || pauses += 1,
        );
        assert_eq!(result.unwrap(), vec![2]);
        assert_eq!((wakes, pauses), (1, 12));
        assert_eq!(
            wake_and_enumerate(
                || Ok(vec![2]),
                || panic!("awake screen must not be woken"),
                || panic!("must not wait")
            )
            .unwrap(),
            vec![2]
        );
    }

    #[test]
    fn headless_and_wake_failure_stop_with_bounded_work() {
        let mut waits = 0;
        assert!(wake_and_enumerate::<u32>(|| Ok(vec![]), || Ok(()), || waits += 1).is_err());
        assert_eq!(waits, 40);
        assert_eq!(
            wake_and_enumerate::<u32>(
                || Ok(vec![]),
                || Err("wake failed".into()),
                || panic!("must not wait")
            )
            .unwrap_err(),
            "wake failed"
        );
        assert_eq!(
            wake_and_enumerate::<u32>(
                || Err("interactive desktop changed".into()),
                || panic!("must not wake inactive session"),
                || {}
            )
            .unwrap_err(),
            "interactive desktop changed"
        );
    }

    fn display(id: u32) -> (u32, Option<DisplayGeometry>) {
        (
            id,
            Some(DisplayGeometry {
                id,
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
                scale: 2.0,
            }),
        )
    }

    #[test]
    fn default_uses_snapshot_order_not_id_or_primary_flag() {
        let (id, geometry) = select(None, || Ok(vec![display(9), display(2)])).unwrap();
        assert_eq!(id, 9);
        assert_eq!(geometry.x, -1920);
        assert_eq!(geometry.scale, 2.0);
        assert_eq!(
            select(Some(2), || Ok(vec![display(9), display(2)]))
                .unwrap()
                .0,
            2
        );
    }

    #[test]
    fn transition_reenumerates_and_uses_new_snapshot() {
        let mut calls = 0;
        let result = select(None, || {
            calls += 1;
            Ok(match calls {
                1 => vec![],
                2 => vec![(9, None), display(2)],
                _ => vec![display(3), display(2)],
            })
        });
        assert_eq!(result.unwrap().0, 3);
        assert_eq!(calls, 3);
    }

    #[test]
    fn unavailable_requested_display_never_falls_back() {
        let mut calls = 0;
        assert!(
            select(Some(9), || {
                calls += 1;
                Ok(vec![display(2)])
            })
            .is_err()
        );
        assert_eq!(calls, 3);
        assert!(select(None, || Ok(vec![(9, None), display(2)])).is_err());
    }

    #[test]
    fn invalid_geometry_and_headless_are_bounded_failures() {
        for (width, height, scale) in [
            (0, 1080, 1.0),
            (1920, 0, 1.0),
            (1920, 1080, f32::NAN),
            (1920, 1080, f32::INFINITY),
            (1920, 1080, 0.0),
        ] {
            let mut d = display(2);
            let g = d.1.as_mut().unwrap();
            g.width = width;
            g.height = height;
            g.scale = scale;
            assert!(select(None, || Ok(vec![d, display(3)])).is_err());
        }
        assert!(select::<u32>(None, || Ok(vec![])).is_err());
        let mut calls = 0;
        assert_eq!(
            select::<u32>(None, || {
                calls += 1;
                Err("enumeration failed".into())
            })
            .unwrap_err(),
            "enumeration failed"
        );
        assert_eq!(calls, 3);
    }
}
