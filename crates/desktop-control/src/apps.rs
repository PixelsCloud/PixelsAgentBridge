//! Runs inside the selected desktop helper, never in a root/SYSTEM service as a
//! substitute for the requested session. Public transport routing is separate.
use pab_protocol::*;

/// Called only in the helper selected by the service. Recheck native identity
/// and the active desktop immediately before dispatch and after the native call.
pub fn query_guarded(
    id: RequestId,
    query: &AppQuery,
    expected: &ExecutionIdentity,
    active: impl Fn() -> bool,
) -> SystemQueryReply {
    let mut reply = SystemQueryReply::pending_kind(id, query.kind());
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    };
    reply.sampled_from_unix_ms = Some(now());
    let verify = || -> Result<(), String> {
        query.validate().map_err(str::to_owned)?;
        expected.validate().map_err(str::to_owned)?;
        if !active() {
            return Err("desktop_session_unavailable: selected desktop is not active".into());
        }
        #[cfg(any(windows, target_os = "macos"))]
        {
            let observed = session_identity()?
                .observation(
                    ExecutionMode::DesktopUser,
                    ExecutionEnvironmentSource::InteractiveSession,
                )
                .map_err(|e| e.to_string())?;
            if &observed != expected {
                return Err("desktop_identity_changed: query execution contexts again".into());
            }
            Ok(())
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        Err(
            "unsupported_platform: application management is unavailable on the headless product"
                .into(),
        )
    };
    let result = verify()
        .map_err(|e| AppActionError::new(false, e))
        .and_then(|()| match query {
            AppQuery::List { request } => list(request)
                .map(|snapshot| AppSnapshot::List { snapshot })
                .map_err(|e| AppActionError::new(false, e)),
            AppQuery::Execute { request } => {
                act(request).map(|result| AppSnapshot::Action { result })
            }
        });
    match result {
        Ok(snapshot) => {
            if let AppSnapshot::List { snapshot } = &snapshot {
                reply.returned_count = snapshot.apps.len() as u32;
                reply.truncated = snapshot.truncated;
                if reply.truncated {
                    reply.stop_reason = Some("application_snapshot_limit".into());
                }
            } else {
                reply.returned_count = 1;
            }
            reply.data = Some(SystemQueryData::Applications { snapshot });
            reply.state = "completed".into();
            if let Err(error) = verify() {
                reply.state = if query.is_mutation() {
                    "unconfirmed"
                } else {
                    "failed"
                }
                .into();
                reply.error = Some(error);
            }
        }
        Err(error) => {
            reply.state = if error.action_started {
                "unconfirmed"
            } else {
                "failed"
            }
            .into();
            reply.error = Some(error.message);
        }
    }
    reply.sampled_at_unix_ms = Some(now());
    reply
}
#[cfg(windows)]
#[path = "apps/windows.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "apps/macos.rs"]
mod native;

#[cfg(any(windows, target_os = "macos"))]
fn session_identity() -> Result<pab_os_control::execution::UserIdentity, String> {
    let identity = pab_os_control::execution::current_identity().map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    let identity = {
        let mut identity = identity;
        identity.session_id = Some(crate::native::active_console_session().ok_or("desktop_session_unavailable: application operations require the active graphical session")?);
        identity
    };
    #[cfg(windows)]
    if identity.session_id.is_none_or(|id| id == 0)
        || matches!(
            identity.account_id.as_str(),
            "S-1-5-18" | "S-1-5-19" | "S-1-5-20"
        )
    {
        return Err("desktop_session_unavailable: application operations require an interactive user helper".into());
    }
    #[cfg(target_os = "macos")]
    if identity.account_id == "uid:0" {
        return Err(
            "desktop_session_unavailable: application operations require a logged-in user helper"
                .into(),
        );
    }
    Ok(identity)
}

/// Invoke only after the parent durably accepts the request. An error with
/// action_started=true must remain unconfirmed, never automatically replayed.
pub fn act(request: &AppActionRequest) -> Result<AppActionResult, AppActionError> {
    let before = |message: String| AppActionError::new(false, message);
    request.validate().map_err(|e| before(e.into()))?;
    #[cfg(any(windows, target_os = "macos"))]
    {
        let identity = session_identity().map_err(before)?;
        if let AppActionRequest::OpenFile { path, .. } = request {
            let p = std::path::Path::new(path);
            if !p.is_absolute() || !p.is_file() {
                return Err(before(
                    "file_unavailable: open_file requires an existing absolute local file path"
                        .into(),
                ));
            }
        }
        native::act(request, &identity)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    Err(before(
        "unsupported_platform: application management is unavailable on the headless product"
            .into(),
    ))
}

/// Bounded discovery snapshot. A new call is a new observation, not a next page.
pub fn list(request: &AppListRequest) -> Result<AppListSnapshot, String> {
    request.validate().map_err(str::to_owned)?;
    #[cfg(any(windows, target_os = "macos"))]
    {
        let identity = session_identity()?;
        let mut snapshot = AppListSnapshot {
            apps: vec![],
            truncated: false,
            sources: vec![],
            warnings: vec![],
            execution_identity: identity
                .observation(
                    ExecutionMode::DesktopUser,
                    ExecutionEnvironmentSource::InteractiveSession,
                )
                .map_err(|e| e.to_string())?,
        };
        native::list(request, &identity, &mut snapshot)?;
        finish(request, &mut snapshot);
        if serde_json::to_vec(&snapshot).map_or(true, |v| v.len() > MAX_SYSTEM_REPLY_BYTES - 1024) {
            return Err("application snapshot identity metadata exceeds response limit".into());
        }
        Ok(snapshot)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    Err(
        "unsupported_platform: application management is unavailable on the headless product"
            .into(),
    )
}

#[cfg(any(windows, target_os = "macos", test))]
fn finish(request: &AppListRequest, snapshot: &mut AppListSnapshot) {
    let search = request.search.to_lowercase();
    snapshot.apps.retain(|app| {
        app.name.to_lowercase().contains(&search)
            || app
                .app_id
                .as_ref()
                .is_some_and(|v| v.to_lowercase().contains(&search))
            || app
                .path
                .as_ref()
                .is_some_and(|v| v.to_lowercase().contains(&search))
    });
    snapshot.apps.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.app_id.cmp(&b.app_id))
            .then(a.path.cmp(&b.path))
    });
    snapshot.truncated |= snapshot.apps.len() > request.limit as usize;
    snapshot.apps.truncate(request.limit as usize);
    // Leave room for the SystemQuery/desktop envelope. IDs and paths are never
    // shortened: an incomplete identifier must not be offered for activation.
    let candidates = std::mem::take(&mut snapshot.apps);
    let mut size = serde_json::to_vec(snapshot).map_or(MAX_SYSTEM_REPLY_BYTES, |v| v.len());
    for app in candidates {
        let bytes = serde_json::to_vec(&app).map_or(MAX_SYSTEM_REPLY_BYTES, |v| v.len()) + 1;
        if size + bytes > MAX_SYSTEM_REPLY_BYTES - 2048 {
            snapshot.truncated = true;
            continue;
        }
        size += bytes;
        snapshot.apps.push(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn application_guard_rejects_inactive_or_mismatched_session_before_native_action() {
        let mut identity = pab_os_control::execution::current_identity()
            .unwrap()
            .observation(
                ExecutionMode::User,
                ExecutionEnvironmentSource::NativeAccount,
            )
            .unwrap();
        identity.mode = ExecutionMode::DesktopUser;
        identity.environment_source = ExecutionEnvironmentSource::InteractiveSession;
        identity.session_id = Some("invalid-session".into());
        let query = AppQuery::Execute {
            request: AppActionRequest::Launch {
                application: AppTarget::Id {
                    id: "fixture.never-launched".into(),
                },
            },
        };
        for active in [false, true] {
            let reply = query_guarded(RequestId::new(), &query, &identity, || active);
            assert_eq!(reply.state, "failed");
            assert!(reply.data.is_none());
            assert!(reply.error.is_some());
        }
    }
    #[test]
    fn discovery_filters_before_limiting_and_bounds_escaped_wire_bytes() {
        let identity = pab_os_control::execution::current_identity()
            .unwrap()
            .observation(
                ExecutionMode::User,
                ExecutionEnvironmentSource::NativeAccount,
            )
            .unwrap();
        let mut snapshot = AppListSnapshot {
            apps: (0..200)
                .map(|i| AppInfo {
                    name: format!("Application {i}"),
                    app_id: Some("\u{1}".repeat(4096)),
                    path: None,
                    source: "fixture".into(),
                    instance: None,
                })
                .collect(),
            truncated: false,
            sources: vec![],
            warnings: vec![],
            execution_identity: identity,
        };
        finish(
            &AppListRequest {
                scope: AppListScope::Installed,
                search: "Application 199".into(),
                limit: 200,
            },
            &mut snapshot,
        );
        assert_eq!(snapshot.apps.len(), 1);
        snapshot.apps = vec![snapshot.apps[0].clone(); 200];
        finish(
            &AppListRequest {
                scope: AppListScope::Installed,
                search: String::new(),
                limit: 200,
            },
            &mut snapshot,
        );
        assert!(snapshot.truncated);
        assert!(serde_json::to_vec(&snapshot).unwrap().len() < MAX_SYSTEM_REPLY_BYTES);
        assert_eq!(snapshot.apps[0].app_id.as_ref().unwrap().len(), 4096);
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn headless_product_rejects_discovery_without_gui_dependencies() {
        assert!(
            list(&AppListRequest {
                scope: AppListScope::Installed,
                search: String::new(),
                limit: 20
            })
            .unwrap_err()
            .starts_with("unsupported_platform:")
        );
        for request in [
            AppActionRequest::Launch {
                application: AppTarget::Id {
                    id: "fixture".into(),
                },
            },
            AppActionRequest::OpenFile {
                path: "/tmp/fixture".into(),
                application: None,
            },
        ] {
            let result = act(&request).unwrap_err();
            assert!(!result.action_started);
            assert!(result.message.starts_with("unsupported_platform:"));
        }
    }
}
