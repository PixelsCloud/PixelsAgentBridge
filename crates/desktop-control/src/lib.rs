//! Product policy around xcap/enigo and native foreign-window operations.
use pab_protocol::*;
use std::collections::HashMap;
mod batch;
#[cfg(windows)]
#[path = "windows.rs"]
mod native;
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "macos/mod.rs"]
mod native;
#[cfg(target_os = "macos")]
pub use native::{
    accessibility_allowed, active_console, apply_input, release_input, require_screen_capture,
    screen_capture_allowed,
};
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
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
    /// Capture the currently referenced window once, encode and drop the source pixels.
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    pub fn capture_window(
        &self,
        options: &ScreenshotOptions,
    ) -> Result<pab_screenshot::EncodedScreenshot, String> {
        options.validate().map_err(str::to_owned)?;
        #[cfg(target_os = "macos")]
        native::require_screen_capture()?;
        check_session()?;
        let reference = options
            .window_ref
            .as_ref()
            .ok_or("window_ref is required")?;
        let e = self
            .windows
            .get(reference)
            .ok_or("window reference is stale or belongs to another helper; list windows again")?;
        native::verify(e.id, e.pid, &self.key, e.marker)?;
        let window = xcap::Window::all()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|w| w.id().ok() == Some(e.id) && w.pid().ok() == Some(e.pid))
            .ok_or("referenced window is unavailable or excluded by xcap; list windows again")?;
        if window.is_minimized().map_err(|e| e.to_string())? {
            return Err("window is minimized; restore it before capture".into());
        }
        let rect = window_rect(&window)?;
        let monitor = window.current_monitor().map_err(|e| e.to_string())?;
        let scale = monitor.scale_factor().map_err(|e| e.to_string())?;
        if !scale.is_finite() || scale <= 0.0 || scale > 8.0 {
            return Err("invalid display scaling".into());
        }
        // xcap may allocate DPI-scaled pixels before returning; a conservative preflight.
        let factor = if cfg!(any(windows, target_os = "macos")) {
            f64::from(scale.max(1.0))
        } else {
            1.0
        };
        if options.mode != ScreenshotMode::Jpeg
            && f64::from(rect.width) * f64::from(rect.height) * factor * factor
                > MAX_SCREENSHOT_PIXELS as f64
        {
            return Err("window exceeds capture pixel budget; resize it before capture".into());
        }
        let id = monitor.id().map_err(|e| e.to_string())?;
        let captured_at = now();
        native::verify(e.id, e.pid, &self.key, e.marker)?;
        let image = window.capture_image().map_err(|e| e.to_string())?;
        native::verify(e.id, e.pid, &self.key, e.marker)?;
        if window.is_minimized().map_err(|e| e.to_string())?
            || window_rect(&window)? != rect
            || window
                .current_monitor()
                .map_err(|e| e.to_string())?
                .id()
                .map_err(|e| e.to_string())?
                != id
            || window
                .current_monitor()
                .map_err(|e| e.to_string())?
                .scale_factor()
                .map_err(|e| e.to_string())?
                != scale
        {
            return Err("window geometry/display changed during capture; retry with fresh window information".into());
        }
        let mut codec_options = options.clone();
        codec_options.window_ref = None;
        let mut encoded = pab_screenshot::encode(
            pab_screenshot::image::DynamicImage::ImageRgba8(image),
            &codec_options,
            Some(id),
            (rect.x, rect.y),
        )?;
        encoded.info.window_ref = Some(reference.clone());
        encoded.info.desktop_rect = Some(rect);
        encoded.info.captured_at_unix_ms = captured_at;
        encoded.info.validate(options).map_err(str::to_owned)?;
        Ok(encoded)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    pub fn capture_window(
        &self,
        _: &ScreenshotOptions,
    ) -> Result<pab_screenshot::EncodedScreenshot, String> {
        Err("window screenshots are unsupported on this platform".into())
    }
    pub fn query(&mut self, id: RequestId, query: &DesktopQuery) -> SystemQueryReply {
        self.query_guarded(id, query, || Ok(()))
    }
    /// The helper checks its active desktop between actions, including while waiting.
    pub fn query_guarded(
        &mut self,
        id: RequestId,
        query: &DesktopQuery,
        mut guard: impl FnMut() -> Result<(), String>,
    ) -> SystemQueryReply {
        let system = SystemQuery::Desktop {
            query: query.clone(),
        };
        let mut reply = SystemQueryReply::pending(id, &system);
        let start = now();
        reply.sampled_from_unix_ms = Some(start);
        let mut snapshot = DesktopSnapshot::new(self.instance.clone(), native::BACKEND);
        let result = query.validate().map_err(str::to_owned).and_then(|_| {
            if let DesktopQuery::Batch {
                window_ref,
                actions,
                timeout_ms,
            } = query
            {
                check_session()?;
                batch::run(
                    window_ref,
                    actions,
                    *timeout_ms,
                    &mut snapshot,
                    &mut guard,
                    |action, step| self.batch_action(window_ref, action, step),
                )
            } else {
                guard()?;
                self.execute(query, &mut snapshot)
            }
        });
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
        reply.returned_count = snapshot.batch.as_ref().map_or(
            (snapshot.monitors.len() + snapshot.windows.len()) as u32,
            |batch| batch.steps.len() as u32,
        );
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
            DesktopQuery::Batch { .. } => unreachable!("batches are dispatched by query_guarded"),
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
                #[cfg(target_os = "macos")]
                native::require_screen_capture()?;
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
    #[cfg(target_os = "macos")]
    if !native::active_console() {
        return Err("macOS desktop requires the active console user".into());
    }
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_none() {
        return Err("desktop operations require an X11 session; Wayland unsupported".into());
    }
    Ok(())
}

#[cfg(all(test, windows))]
#[path = "windows_tests.rs"]
mod windows_tests;

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn window_rect(window: &xcap::Window) -> Result<ScreenshotDesktopRect, String> {
    let rect = ScreenshotDesktopRect {
        x: window.x().map_err(|e| e.to_string())?,
        y: window.y().map_err(|e| e.to_string())?,
        width: window.width().map_err(|e| e.to_string())?,
        height: window.height().map_err(|e| e.to_string())?,
    };
    if rect.width == 0
        || rect.height == 0
        || rect.width > 65535
        || rect.height > 65535
        || i64::from(rect.x) + i64::from(rect.width) > i64::from(i32::MAX)
        || i64::from(rect.y) + i64::from(rect.height) > i64::from(i32::MAX)
    {
        return Err("window coordinates/dimensions exceed capture budget".into());
    }
    Ok(rect)
}
