use super::*;
use std::time::{Duration, Instant};

/// One helper request owns the sequence. Errors stop subsequent actions; no replay or rollback.
pub(super) fn run(
    reference: &str,
    actions: &[DesktopAction],
    timeout_ms: u32,
    snapshot: &mut DesktopSnapshot,
    guard: &mut impl FnMut() -> Result<(), String>,
    mut execute: impl FnMut(&DesktopAction, &mut DesktopSnapshot) -> Result<(), String>,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms));
    let mut report = DesktopBatchReport {
        steps: actions
            .iter()
            .enumerate()
            .map(|(i, a)| DesktopBatchStep {
                index: i as u32,
                kind: a.kind().into(),
                state: "skipped".into(),
                action_started: false,
                verification: None,
                error: None,
            })
            .collect(),
        completed_steps: 0,
        failed_step: None,
    };
    snapshot.window_ref = Some(reference.into());
    let mut failure = None;
    for (index, action) in actions.iter().enumerate() {
        let mut step = DesktopSnapshot::new(snapshot.helper_instance.clone(), &snapshot.backend);
        let outcome: Result<(), String> = (|| {
            guard()?;
            if Instant::now() >= deadline {
                return Err("batch deadline reached before action".into());
            }
            if let DesktopAction::Wait { ms } = action {
                let until = Instant::now() + Duration::from_millis(u64::from(*ms));
                while Instant::now() < until {
                    guard()?;
                    if Instant::now() >= deadline {
                        return Err("batch deadline reached during wait".into());
                    }
                    std::thread::sleep(
                        Duration::from_millis(10)
                            .min(until.saturating_duration_since(Instant::now())),
                    );
                }
                step.verification = Some("wait_elapsed".into());
            } else {
                execute(action, &mut step)?;
            }
            guard()?;
            Ok(())
        })();
        snapshot.action_started |= step.action_started;
        let item = &mut report.steps[index];
        item.action_started = step.action_started;
        item.verification = step.verification;
        match outcome {
            Ok(()) => {
                item.state = "completed".into();
                report.completed_steps += 1;
            }
            Err(error) => {
                item.state = if step.action_started {
                    "unconfirmed"
                } else {
                    "failed"
                }
                .into();
                item.error = Some(short(&error, 256));
                report.failed_step = Some(index as u32);
                failure = Some(error);
                break;
            }
        }
    }
    snapshot.batch = Some(report);
    snapshot.verification = Some(
        "ordered actions; no rollback; input API acceptance is not application verification".into(),
    );
    failure.map_or(Ok(()), Err)
}

impl DesktopSession {
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    pub(super) fn batch_action(
        &mut self,
        reference: &str,
        action: &DesktopAction,
        snapshot: &mut DesktopSnapshot,
    ) -> Result<(), String> {
        let query = match action {
            DesktopAction::Focus {} => Some(DesktopQuery::Focus {
                window_ref: reference.into(),
            }),
            DesktopAction::Control { control } => Some(DesktopQuery::Control {
                window_ref: reference.into(),
                control: *control,
            }),
            DesktopAction::TypeText { text } => Some(DesktopQuery::TypeText {
                window_ref: reference.into(),
                text: text.clone(),
            }),
            _ => None,
        };
        if let Some(query) = query {
            return self.execute(&query, snapshot);
        }
        let e = self
            .windows
            .get(reference)
            .ok_or("window reference is stale; list windows again")?;
        let verify = || -> Result<(), String> {
            native::verify(e.id, e.pid, &self.key, e.marker)?;
            if !native::focused(e.id)? {
                return Err("target is not foreground; batch stopped".into());
            }
            Ok(())
        };
        verify()?;
        super::on_input_thread(|| {
            use enigo::{Axis, Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
            let mut engine = Enigo::new(&Settings {
                open_prompt_to_get_permissions: false,
                ..Settings::default()
            })
            .map_err(|e| e.to_string())?;
            verify()?;
            match action {
                DesktopAction::KeyChord { modifiers, key } => {
                    let key = keyboard_key(key)?;
                    let mut keys: Vec<_> = modifiers
                        .iter()
                        .map(|modifier| match modifier {
                            DesktopModifier::Control => enigo::Key::Control,
                            DesktopModifier::Alt => enigo::Key::Alt,
                            DesktopModifier::Shift => enigo::Key::Shift,
                            DesktopModifier::Meta => enigo::Key::Meta,
                        })
                        .collect();
                    keys.push(key);
                    press_and_release(
                        &keys,
                        verify,
                        |key, down| {
                            engine
                                .key(
                                    key,
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
                DesktopAction::Click { x, y, button } => {
                    let window = xcap::Window::all()
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .find(|w| w.id().ok() == Some(e.id) && w.pid().ok() == Some(e.pid))
                        .ok_or("window is unavailable for click coordinates")?;
                    if window.is_minimized().map_err(|e| e.to_string())? {
                        return Err("window is minimized".into());
                    }
                    let rect = window_rect(&window)?;
                    if u32::from(*x) >= rect.width || u32::from(*y) >= rect.height {
                        return Err("click is outside the window rectangle".into());
                    }
                    snapshot.action_started = true;
                    engine
                        .move_mouse(
                            rect.x
                                .checked_add(i32::from(*x))
                                .ok_or("click x overflow")?,
                            rect.y
                                .checked_add(i32::from(*y))
                                .ok_or("click y overflow")?,
                            Coordinate::Abs,
                        )
                        .map_err(|e| e.to_string())?;
                    verify()?;
                    if window_rect(&window)? != rect {
                        return Err("window moved before click; batch stopped".into());
                    }
                    if !native::pointer_targets_window(e.id)? {
                        return Err("pointer targets another window; click stopped".into());
                    }
                    let button = match button {
                        DesktopMouseButton::Left => Button::Left,
                        DesktopMouseButton::Right => Button::Right,
                        DesktopMouseButton::Middle => Button::Middle,
                    };
                    press_and_release(
                        &[button],
                        verify,
                        |button, down| {
                            engine
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
                DesktopAction::Scroll { axis, amount } => {
                    if !native::pointer_targets_window(e.id)? {
                        return Err(
                            "pointer must be over the referenced window before scrolling".into(),
                        );
                    }
                    snapshot.action_started = true;
                    engine
                        .scroll(
                            i32::from(*amount),
                            match axis {
                                DesktopScrollAxis::Horizontal => Axis::Horizontal,
                                DesktopScrollAxis::Vertical => Axis::Vertical,
                            },
                        )
                        .map_err(|e| e.to_string())?;
                }
                _ => return Err("unsupported batch input action".into()),
            }
            verify()?;
            snapshot.verification =
                Some("input_api_accepted; application effect is not verified".into());
            Ok(())
        })
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    pub(super) fn batch_action(
        &mut self,
        _: &str,
        _: &DesktopAction,
        _: &mut DesktopSnapshot,
    ) -> Result<(), String> {
        Err("desktop operations unsupported on this platform".into())
    }
}

/// Release even a failed press (which may be partial), in reverse order, without short-circuiting cleanup.
fn press_and_release<T: Copy>(
    keys: &[T],
    mut verify: impl FnMut() -> Result<(), String>,
    mut send: impl FnMut(T, bool) -> Result<(), String>,
    started: &mut bool,
) -> Result<(), String> {
    let mut held = Vec::new();
    let outcome = (|| {
        for key in keys {
            verify()?;
            *started = true;
            held.push(*key);
            send(*key, true)?;
        }
        Ok(())
    })();
    let mut cleanup = None;
    for key in held.into_iter().rev() {
        if let Err(error) = send(key, false) {
            cleanup = Some(error);
        }
    }
    match (outcome, cleanup) {
        (Ok(()), None) => Ok(()),
        (Err(error), None) => Err(error),
        (Ok(()), Some(error)) => Err(format!("input release failed: {error}")),
        (Err(error), Some(cleanup)) => Err(format!("{error}; input release failed: {cleanup}")),
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn keyboard_key(name: &str) -> Result<enigo::Key, String> {
    use enigo::Key::*;
    Ok(match name {
        "enter" => Return,
        "tab" => Tab,
        "escape" => Escape,
        "space" => Space,
        "backspace" => Backspace,
        "delete" => Delete,
        "left" => LeftArrow,
        "right" => RightArrow,
        "up" => UpArrow,
        "down" => DownArrow,
        "home" => Home,
        "end" => End,
        "page_up" => PageUp,
        "page_down" => PageDown,
        "f1" => F1,
        "f2" => F2,
        "f3" => F3,
        "f4" => F4,
        "f5" => F5,
        "f6" => F6,
        "f7" => F7,
        "f8" => F8,
        "f9" => F9,
        "f10" => F10,
        "f11" => F11,
        "f12" => F12,
        s if valid_desktop_key(s) && s.len() == 1 => Unicode(s.chars().next().unwrap()),
        _ => return Err("unsupported key".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> DesktopSnapshot {
        DesktopSnapshot::new("test".into(), "fake")
    }
    #[test]
    fn ordered_partial_failure_skips_remaining_and_retains_completed_evidence() {
        let mut s = snapshot();
        let actions = [
            DesktopAction::Focus {},
            DesktopAction::TypeText {
                text: "private".into(),
            },
            DesktopAction::Focus {},
        ];
        let mut calls = vec![];
        assert!(
            run("ref", &actions, 1000, &mut s, &mut || Ok(()), |a, step| {
                calls.push(a.kind());
                step.action_started = true;
                if calls.len() == 2 {
                    Err("input failure".into())
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        assert_eq!(calls, ["focus", "type_text"]);
        let report = s.batch.as_ref().unwrap();
        assert_eq!(report.completed_steps, 1);
        assert_eq!(report.failed_step, Some(1));
        assert_eq!(
            report
                .steps
                .iter()
                .map(|s| s.state.as_str())
                .collect::<Vec<_>>(),
            ["completed", "unconfirmed", "skipped"]
        );
        assert!(!serde_json::to_string(&s).unwrap().contains("private"));
    }
    #[test]
    fn deadline_stops_next_step_after_slow_native_call_without_claiming_rollback() {
        let mut s = snapshot();
        let mut calls = 0;
        assert!(
            run(
                "ref",
                &[DesktopAction::Focus {}, DesktopAction::Focus {}],
                100,
                &mut s,
                &mut || Ok(()),
                |_, step| {
                    calls += 1;
                    step.action_started = true;
                    std::thread::sleep(Duration::from_millis(150));
                    Ok(())
                }
            )
            .is_err()
        );
        assert_eq!(calls, 1);
        let report = s.batch.unwrap();
        assert_eq!(report.completed_steps, 1);
        assert_eq!(report.steps[1].state, "failed");
        assert!(report.steps[1].error.as_ref().unwrap().contains("deadline"));
    }
    #[test]
    fn desktop_change_during_wait_stops_batch_without_sending_input() {
        let mut s = snapshot();
        let mut checks = 0;
        assert!(
            run(
                "ref",
                &[DesktopAction::Wait { ms: 100 }, DesktopAction::Focus {}],
                1000,
                &mut s,
                &mut || {
                    checks += 1;
                    if checks >= 3 {
                        Err("desktop changed".into())
                    } else {
                        Ok(())
                    }
                },
                |_, _| panic!("input must not run")
            )
            .is_err()
        );
        let report = s.batch.unwrap();
        assert_eq!(report.completed_steps, 0);
        assert_eq!(report.steps[1].state, "skipped");
        assert!(!s.action_started);
    }
    #[test]
    fn malformed_batch_rejected_before_first_action() {
        let mut session = DesktopSession::new();
        let result = session.query(
            RequestId::new(),
            &DesktopQuery::Batch {
                window_ref: RequestId::new().to_string(),
                actions: vec![
                    DesktopAction::Focus {},
                    DesktopAction::TypeText { text: "".into() },
                ],
                timeout_ms: 5000,
            },
        );
        assert_eq!(result.state, "failed");
        let Some(SystemQueryData::Desktop { snapshot }) = result.data else {
            panic!()
        };
        assert!(!snapshot.action_started);
        assert!(snapshot.batch.is_none());
    }
    #[test]
    fn failed_press_releases_all_attempted_keys_in_reverse_even_when_release_fails() {
        let mut sent = vec![];
        let mut started = false;
        let error = press_and_release(
            &[1, 2, 3],
            || Ok(()),
            |key, down| {
                sent.push((key, down));
                match (key, down) {
                    (2, true) => Err("press".into()),
                    (2, false) => Err("release".into()),
                    _ => Ok(()),
                }
            },
            &mut started,
        )
        .unwrap_err();
        assert!(started);
        assert_eq!(sent, [(1, true), (2, true), (2, false), (1, false)]);
        assert!(error.contains("press") && error.contains("release"));
    }
    #[test]
    fn foreground_change_stops_pressing_but_still_releases_held_modifiers() {
        let mut checks = 0;
        let mut sent = vec![];
        assert!(
            press_and_release(
                &[1, 2],
                || {
                    checks += 1;
                    if checks == 2 {
                        Err("focus changed".into())
                    } else {
                        Ok(())
                    }
                },
                |key, down| {
                    sent.push((key, down));
                    Ok(())
                },
                &mut false
            )
            .is_err()
        );
        assert_eq!(sent, [(1, true), (1, false)]);
    }
}
