use super::*;

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(super) fn target(m: &xcap::Monitor, instance: &str) -> Result<MonitorTarget, String> {
    let scale = m.scale_factor().map_err(|e| e.to_string())?;
    let rotation = m.rotation().map_err(|e| e.to_string())?;
    if !scale.is_finite()
        || !(0.01..=8.0).contains(&scale)
        || !rotation.is_finite()
        || !(0.0..=360.0).contains(&rotation)
    {
        return Err("invalid monitor scaling/rotation".into());
    }
    let target = MonitorTarget {
        helper_instance: instance.into(),
        id: m.id().map_err(|e| e.to_string())?,
        x: m.x().map_err(|e| e.to_string())?,
        y: m.y().map_err(|e| e.to_string())?,
        width: m.width().map_err(|e| e.to_string())?,
        height: m.height().map_err(|e| e.to_string())?,
        scale_percent: (scale * 100.0).round() as u32,
        rotation_degrees: rotation.round() as u16,
        coordinate_space: if cfg!(target_os = "macos") {
            MonitorCoordinateSpace::LogicalPoints
        } else {
            MonitorCoordinateSpace::PhysicalPixels
        },
    };
    target.validate().map_err(str::to_owned)?;
    Ok(target)
}

fn verify_target(expected: &MonitorTarget, actual: Option<&MonitorTarget>) -> Result<(), String> {
    if actual != Some(expected) {
        return Err("monitor/session geometry changed or target is unavailable; list monitors again; input not replayed".into());
    }
    Ok(())
}

impl DesktopSession {
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    pub(super) fn monitor_input(
        &self,
        input: &MonitorInput,
        snapshot: &mut DesktopSnapshot,
        guard: &mut (impl FnMut() -> Result<(), String> + Send),
    ) -> Result<(), String> {
        input.validate().map_err(str::to_owned)?;
        guard()?;
        check_session()?;
        #[cfg(target_os = "macos")]
        {
            macos_display::active_monitors()?;
        } // Wake/settle on the worker, never on AppKit's main thread.
        let verify = || {
            check_session()?;
            let monitors = xcap::Monitor::all().map_err(|e| e.to_string())?;
            let actual = monitors
                .iter()
                .find(|m| m.id().ok() == Some(input.target.id))
                .map(|m| target(m, &self.instance))
                .transpose()?;
            verify_target(&input.target, actual.as_ref())
        };
        verify()?;
        let (x, y) = input.action.point();
        let position = input.target.native_point(x, y).map_err(str::to_owned)?;
        on_input_thread(|| {
            use enigo::{Button, Coordinate, Direction};
            #[cfg(not(target_os = "macos"))]
            use enigo::{Enigo, Mouse, Settings};
            #[cfg(target_os = "macos")]
            let mut engine = native::recovery::RecoveryInput::new()?;
            #[cfg(not(target_os = "macos"))]
            let mut engine = Enigo::new(&Settings {
                open_prompt_to_get_permissions: false,
                ..Settings::default()
            })
            .map_err(|e| e.to_string())?;
            guard()?;
            verify()?;
            snapshot.action_started = true;
            engine
                .move_mouse(position.0, position.1, Coordinate::Abs)
                .map_err(|e| e.to_string())?;
            // Posting input is asynchronous. Bounded observation, never repeat the move/click.
            let mut reached = false;
            for _ in 0..20 {
                guard()?;
                verify()?;
                if engine.location().map_err(|e| e.to_string())? == position {
                    reached = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            if !reached {
                return Err(
                    "pointer did not reach requested monitor point; click was not sent".into(),
                );
            }
            let engine = std::cell::RefCell::new(engine);
            if let MonitorAction::Click { button, .. } = input.action {
                let button = match button {
                    DesktopMouseButton::Left => Button::Left,
                    DesktopMouseButton::Right => Button::Right,
                    DesktopMouseButton::Middle => Button::Middle,
                };
                batch::press_and_release(
                    &[button],
                    || {
                        guard()?;
                        verify()?;
                        if engine.borrow().location().map_err(|e| e.to_string())? != position {
                            return Err("pointer moved before click; input stopped".into());
                        }
                        Ok(())
                    },
                    |button, down| {
                        engine
                            .borrow_mut()
                            .button(
                                button,
                                if down {
                                    Direction::Press
                                } else {
                                    Direction::Release
                                },
                            )
                            .map_err(|e| e.to_string())
                    },
                    &mut snapshot.action_started,
                )?;
            }
            guard()?;
            verify()?;
            snapshot.verification = Some(format!(
                "pointer observed at native desktop ({}, {}); input API accepted; application effect not verified",
                position.0, position.1
            ));
            Ok(())
        })
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    pub(super) fn monitor_input(
        &self,
        _: &MonitorInput,
        _: &mut DesktopSnapshot,
        _: &mut (impl FnMut() -> Result<(), String> + Send),
    ) -> Result<(), String> {
        Err("monitor input unsupported on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removal_reuse_geometry_scale_and_helper_changes_stop_input() {
        let t = MonitorTarget {
            helper_instance: RequestId::new().to_string(),
            id: 1,
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
            scale_percent: 100,
            rotation_degrees: 0,
            coordinate_space: MonitorCoordinateSpace::PhysicalPixels,
        };
        assert!(verify_target(&t, Some(&t)).is_ok());
        assert!(verify_target(&t, None).is_err());
        for field in 0..8 {
            let mut changed = t.clone();
            match field {
                0 => changed.id = 2,
                1 => changed.x = 0,
                2 => changed.y = -1080,
                3 => changed.width = 2560,
                4 => changed.height = 1440,
                5 => changed.scale_percent = 150,
                6 => changed.rotation_degrees = 90,
                _ => changed.helper_instance = RequestId::new().to_string(),
            }
            assert!(verify_target(&t, Some(&changed)).is_err());
        }
    }
}
