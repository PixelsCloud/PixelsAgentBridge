//! Product policy around xcap/enigo and native foreign-window operations.
use pab_protocol::*;
use std::collections::HashMap;
#[cfg(windows)]
#[path = "windows.rs"]
mod native;
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod native;
#[cfg(not(any(windows, target_os = "linux")))]
#[path = "unsupported.rs"]
mod native;

struct Entry {
    id: u32,
    pid: u32,
    marker: u32,
}
pub struct DesktopSession {
    instance: String,
    key: String,
    next: u32,
    windows: HashMap<String, Entry>,
}
impl Default for DesktopSession {
    fn default() -> Self {
        Self::new()
    }
}
impl DesktopSession {
    pub fn new() -> Self {
        let instance = RequestId::new().to_string();
        Self {
            key: format!("PixelsAgentBridge_{instance}"),
            instance,
            next: 1,
            windows: HashMap::new(),
        }
    }
    pub fn query(&mut self, id: RequestId, query: &DesktopQuery) -> SystemQueryReply {
        let system = SystemQuery::Desktop {
            query: query.clone(),
        };
        let mut reply = SystemQueryReply::pending(id, &system);
        let start = now();
        reply.sampled_from_unix_ms = Some(start);
        let mut snapshot = DesktopSnapshot::new(self.instance.clone(), native::BACKEND);
        let result = query
            .validate()
            .map_err(str::to_owned)
            .and_then(|_| self.execute(query, &mut snapshot));
        reply.state = if result.is_ok() {
            "completed"
        } else {
            "failed"
        }
        .into();
        reply.error = result.err().map(|s| short(&s, 1024));
        if reply.error.is_some() && snapshot.action_started {
            snapshot.verification =
                Some("unconfirmed; re-list windows/check application; never replay this ID".into());
        }
        reply.returned_count = (snapshot.monitors.len() + snapshot.windows.len()) as u32;
        reply.sampled_at_unix_ms = Some(now());
        reply.data = Some(SystemQueryData::Desktop { snapshot });
        while serde_json::to_vec(&reply).is_ok_and(|v| v.len() > MAX_SYSTEM_REPLY_BYTES) {
            let Some(SystemQueryData::Desktop { snapshot }) = reply.data.as_mut() else {
                break;
            };
            if !snapshot.windows.is_empty() {
                snapshot.windows.pop();
            } else if !snapshot.monitors.is_empty() {
                snapshot.monitors.pop();
            } else {
                break;
            }
            reply.returned_count = (snapshot.monitors.len() + snapshot.windows.len()) as u32;
            reply.truncated = true;
            reply.stop_reason = Some("result_byte_budget".into());
        }
        if let Some(SystemQueryData::Desktop { snapshot }) = &reply.data
            && matches!(
                snapshot.verification.as_deref(),
                Some("window_limit" | "monitor_limit")
            )
        {
            reply.truncated = true;
            reply
                .stop_reason
                .get_or_insert(snapshot.verification.clone().unwrap());
        }
        reply
    }
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    fn execute(
        &mut self,
        query: &DesktopQuery,
        snapshot: &mut DesktopSnapshot,
    ) -> Result<(), String> {
        check_session()?;
        match query {
            DesktopQuery::Monitors {} => {
                let monitors = xcap::Monitor::all().map_err(|e| e.to_string())?;
                let overflow = monitors.len() > 32;
                for m in monitors.into_iter().take(32) {
                    snapshot.monitors.push(MonitorInfo {
                        id: m.id().map_err(|e| e.to_string())?,
                        name: short(&m.name().map_err(|e| e.to_string())?, 256),
                        x: m.x().map_err(|e| e.to_string())?,
                        y: m.y().map_err(|e| e.to_string())?,
                        width: m.width().map_err(|e| e.to_string())?,
                        height: m.height().map_err(|e| e.to_string())?,
                        primary: m.is_primary().map_err(|e| e.to_string())?,
                        scale_percent: m
                            .scale_factor()
                            .ok()
                            .filter(|v| v.is_finite() && *v > 0.0)
                            .map(|v| (v * 100.0).round() as u32),
                        rotation_degrees: m
                            .rotation()
                            .ok()
                            .filter(|v| v.is_finite() && (0.0..=360.0).contains(v))
                            .map(|v| v.round() as u16),
                    });
                }
                if overflow {
                    snapshot.verification = Some("monitor_limit".into());
                }
                if snapshot.monitors.is_empty() {
                    return Err("no interactive monitors available".into());
                }
            }
            DesktopQuery::Windows {} => {
                self.prune();
                let windows = xcap::Window::all().map_err(|e| e.to_string())?;
                for w in windows {
                    let Ok(title) = w.title() else {
                        continue;
                    };
                    if title.trim().is_empty() {
                        continue;
                    }
                    let Ok(id) = w.id() else {
                        continue;
                    };
                    let Ok(pid) = w.pid() else {
                        continue;
                    };
                    if snapshot.windows.len() >= MAX_WINDOW_ENTRIES {
                        snapshot.verification = Some("window_limit".into());
                        break;
                    }
                    // A window can disappear between xcap enumeration and property reads.
                    // Skip only that entry, rather than failing the entire snapshot.
                    let info = (|| -> Result<DesktopWindowInfo, xcap::XCapError> {
                        Ok(DesktopWindowInfo {
                            window_ref: None,
                            control_error: None,
                            title: short(&title, 256),
                            process_id: pid,
                            x: w.x()?,
                            y: w.y()?,
                            width: w.width()?,
                            height: w.height()?,
                            minimized: w.is_minimized()?,
                            maximized: w.is_maximized()?,
                            focused: w.is_focused()?,
                            monitor_id: w.current_monitor().ok().and_then(|m| m.id().ok()),
                        })
                    })();
                    let Ok(mut info) = info else {
                        continue;
                    };
                    let reference = self.reference(id, pid);
                    info.window_ref = reference.as_ref().ok().cloned();
                    info.control_error = reference.err().map(|s| short(&s, 256));
                    snapshot.windows.push(info);
                }
            }
            DesktopQuery::Focus { window_ref }
            | DesktopQuery::Control { window_ref, .. }
            | DesktopQuery::TypeText { window_ref, .. } => {
                let e = self.windows.get(window_ref).ok_or(
                    "window reference is stale or belongs to another helper; list windows again",
                )?;
                native::verify(e.id, e.pid, &self.key, e.marker)?;
                snapshot.window_ref = Some(window_ref.clone());
                snapshot.action_started = true;
                match query {
                    DesktopQuery::TypeText { text, .. } => {
                        // Do not implicitly steal focus: the caller must focus this reference first.
                        if !native::focused(e.id)? {
                            snapshot.action_started = false;
                            return Err(
                                "target is not foreground; call pab_focus_window first".into()
                            );
                        }
                        use enigo::{Enigo, Keyboard, Settings};
                        let mut engine =
                            Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
                        native::verify(e.id, e.pid, &self.key, e.marker)?;
                        if !native::focused(e.id)? {
                            snapshot.action_started = false;
                            return Err("foreground changed before text input".into());
                        }
                        engine.text(text).map_err(|e| e.to_string())?;
                        native::verify(e.id, e.pid, &self.key, e.marker)?;
                        if !native::focused(e.id)? {
                            return Err(
                                "foreground changed during text input; input may be partial".into(),
                            );
                        }
                        snapshot.verification =
                            Some("input_api_accepted; application text is not verified".into());
                    }
                    DesktopQuery::Focus { .. } => {
                        native::act(e.id, e.pid, &self.key, e.marker, None)?;
                        snapshot.verification = Some("foreground_observed".into());
                    }
                    DesktopQuery::Control { control, .. } => {
                        native::act(e.id, e.pid, &self.key, e.marker, Some(*control))?;
                        snapshot.verification = Some("requested_window_state_observed".into());
                    }
                    _ => unreachable!(),
                }
            }
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    fn execute(&mut self, _: &DesktopQuery, _: &mut DesktopSnapshot) -> Result<(), String> {
        Err("desktop operations unsupported on this platform".into())
    }
    fn reference(&mut self, id: u32, pid: u32) -> Result<String, String> {
        if let Some((reference, _)) = self.windows.iter().find(|(_, e)| {
            e.id == id && e.pid == pid && native::verify(e.id, e.pid, &self.key, e.marker).is_ok()
        }) {
            return Ok(reference.clone());
        }
        if self.windows.len() >= 256 {
            return Err("window reference budget reached; reconnect desktop helper".into());
        }
        let marker = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or("window reference generation exhausted")?;
        native::mark(id, pid, &self.key, marker)?;
        let reference = RequestId::new().to_string();
        self.windows
            .insert(reference.clone(), Entry { id, pid, marker });
        Ok(reference)
    }
    fn prune(&mut self) {
        self.windows
            .retain(|_, e| native::verify(e.id, e.pid, &self.key, e.marker).is_ok());
    }
}
impl Drop for DesktopSession {
    fn drop(&mut self) {
        for e in self.windows.values() {
            native::unmark(e.id, e.pid, &self.key, e.marker);
        }
    }
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn short(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
fn check_session() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_none() {
        return Err("desktop operations require an X11 session; Wayland unsupported".into());
    }
    Ok(())
}

#[cfg(all(test, windows))]
#[path = "windows_tests.rs"]
mod windows_tests;
