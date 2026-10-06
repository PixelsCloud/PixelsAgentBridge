//! Native lifecycle operations. Unsafe Windows bindings are isolated in this crate.
use pab_protocol::*;
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

pub mod execution;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

static ACTIVE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
struct Guard(String);
impl Guard {
    fn acquire(key: String) -> Result<Self, String> {
        if !ACTIVE
            .lock()
            .map_err(|_| "resource lock unavailable")?
            .insert(key.clone())
        {
            return Err(
                "resource_busy: another lifecycle operation is active for this resource".into(),
            );
        }
        Ok(Self(key))
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE.lock() {
            active.remove(&self.0);
        }
    }
}
pub fn process_identity(pid: u32) -> Result<String, String> {
    #[cfg(windows)]
    {
        windows::identity(pid)
    }
    #[cfg(target_os = "linux")]
    {
        linux::identity(pid)
    }
    #[cfg(target_os = "macos")]
    {
        macos::identity(pid)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        Err("unsupported process identity backend".into())
    }
}
pub async fn execute(query: &SystemQuery) -> Result<SystemQueryData, String> {
    query.validate().map_err(str::to_owned)?;
    let key = match query {
        SystemQuery::TerminateProcess { pid, .. } => Some(format!("process:{pid}")),
        SystemQuery::ServiceControl { name, .. } => Some(format!(
            "service:{}",
            if cfg!(windows) {
                name.to_lowercase()
            } else {
                name.clone()
            }
        )),
        _ => None,
    };
    let guard = key.map(Guard::acquire).transpose()?;
    #[cfg(windows)]
    {
        let query = query.clone();
        // The guard belongs to the blocking worker, including if its async caller is dropped.
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            windows::execute(&query)
        })
        .await
        .map_err(|_| "native control worker stopped unexpectedly".to_string())?
    }
    #[cfg(target_os = "linux")]
    {
        if matches!(query, SystemQuery::TerminateProcess { .. }) {
            let query = query.clone();
            tokio::task::spawn_blocking(move || {
                let _guard = guard;
                linux::terminate(&query)
            })
            .await
            .map_err(|_| "native control worker stopped unexpectedly".to_string())?
        } else {
            let _guard = guard;
            linux::execute(query).await
        }
    }
    #[cfg(target_os = "macos")]
    {
        let query = query.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            macos::execute(&query)
        })
        .await
        .map_err(|e| format!("macOS control worker: {e}"))?
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = guard;
        Err("unsupported service/process control backend".into())
    }
}
fn bounded(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}
fn empty_service(backend: &str, name: &str) -> ServiceInfo {
    ServiceInfo {
        backend: backend.into(),
        name: name.into(),
        display_name: None,
        state: "unknown".into(),
        sub_state: None,
        start_mode: None,
        pid: None,
        executable: None,
        account: None,
        exit_code: None,
        checkpoint: None,
        wait_hint_ms: None,
        errors: vec![],
    }
}
fn control_result(name: &str, control: ServiceControlAction) -> ServiceControlResult {
    ServiceControlResult {
        name: name.into(),
        control,
        outcome: "completed".into(),
        phase: "accepted".into(),
        changed: false,
        job_path: None,
        service: None,
        error: None,
    }
}
pub fn filter_services(
    entries: &mut Vec<ServiceInfo>,
    name: Option<&str>,
    state: Option<&str>,
    limit: usize,
) -> bool {
    entries.retain(|e| {
        name.is_none_or(|n| {
            e.name.to_lowercase().contains(&n.to_lowercase())
                || e.display_name
                    .as_ref()
                    .is_some_and(|v| v.to_lowercase().contains(&n.to_lowercase()))
        }) && state.is_none_or(|s| s == e.state)
    });
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_lock_rejects_concurrent_control_and_releases_on_drop() {
        let key = "test-resource".to_string();
        let held = Guard::acquire(key.clone()).unwrap();
        assert!(Guard::acquire(key.clone()).is_err());
        drop(held);
        assert!(Guard::acquire(key).is_ok());
    }
    #[test]
    fn list_filters_before_limit_and_sort() {
        let mut list = vec![
            empty_service("test", "bbb"),
            empty_service("test", "aaa"),
            empty_service("test", "other"),
        ];
        list[0].state = "running".into();
        list[1].state = "running".into();
        assert!(filter_services(&mut list, None, Some("running"), 1));
        assert_eq!(list[0].name, "aaa");
    }
}
