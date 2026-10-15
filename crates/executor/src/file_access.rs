//! Bounded, read-only checks of the three user folders that our file tools use.
//! This observes usable access, not the macOS Full Disk Access switch. Never
//! inspect TCC databases, scan directory contents or report user filenames.
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::Semaphore,
};

const FOLDERS: [&str; 3] = ["Downloads", "Documents", "Desktop"];
const DEADLINE: Duration = Duration::from_secs(2);

/// Use native account inventory, not a possibly overwritten HOME variable.
pub fn current_user_home() -> Result<PathBuf, String> {
    pab_os_control::execution::process_user_identity(std::process::id())
        .map(|i| i.home)
        .map_err(|e| e.to_string())
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessState {
    Normal,
    Restricted,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderAccess {
    pub folder: String,
    pub state: AccessState,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAccessReport {
    pub state: AccessState,
    pub folders: Vec<FolderAccess>,
    pub checked_at_unix_ms: u64,
    pub source: String,
}

fn observe(home: &Path, folder: &str) -> FolderAccess {
    let result = std::fs::read_dir(home.join(folder))
        .and_then(|mut entries| entries.next().transpose().map(|_| ()));
    let (state, reason) = classify(result);
    FolderAccess {
        folder: folder.into(),
        state,
        reason,
    }
}

fn classify(result: io::Result<()>) -> (AccessState, Option<String>) {
    match result {
        Ok(()) => (AccessState::Normal, None),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            (AccessState::Restricted, Some("access_denied".into()))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            (AccessState::Failed, Some("directory_missing".into()))
        }
        Err(_) => (AccessState::Failed, Some("directory_check_failed".into())),
    }
}

/// Called before GUI/service startup. Only one fixed folder is checked per
/// worker, so a blocked privacy/filesystem call cannot hide the other folders.
pub fn worker_main() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(2).collect();
    if args.len() != 2 {
        return Err("invalid file access worker arguments".into());
    }
    let home = PathBuf::from(&args[0]);
    let folder = args[1]
        .to_str()
        .filter(|v| FOLDERS.contains(v))
        .ok_or("unknown file access folder")?;
    if !home.is_absolute() {
        return Err("file access home must be absolute".into());
    }
    println!(
        "{}",
        serde_json::to_string(&observe(&home, folder)).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn report(folders: Vec<FolderAccess>) -> FileAccessReport {
    let state = if folders.iter().any(|f| f.state == AccessState::Failed) {
        AccessState::Failed
    } else if folders.iter().any(|f| f.state == AccessState::Restricted) {
        AccessState::Restricted
    } else {
        AccessState::Normal
    };
    FileAccessReport {
        state,
        folders,
        checked_at_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        source: "directory_enumeration".into(),
    }
}

fn failed(folder: &str, reason: &str) -> FolderAccess {
    FolderAccess {
        folder: folder.into(),
        state: AccessState::Failed,
        reason: Some(reason.into()),
    }
}

/// One check per process at a time, including any unreaped timed-out child.
/// Dropping a caller does not drop this supervisor or release its worker slot.
pub async fn check(executable: PathBuf, home: PathBuf) -> Result<FileAccessReport, String> {
    check_with_deadline(executable, home, DEADLINE).await
}

async fn check_with_deadline(
    executable: PathBuf,
    home: PathBuf,
    deadline: Duration,
) -> Result<FileAccessReport, String> {
    static SLOT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let permit = SLOT
        .get_or_init(|| Arc::new(Semaphore::new(1)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| "file access check already running")?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _permit = permit;
        let mut folders = Vec::new();
        let mut unreaped = None;
        for folder in FOLDERS {
            if unreaped.is_some() {
                folders.push(failed(folder, "worker_cleanup_pending"));
                continue;
            }
            let mut command = Command::new(&executable);
            command.args([
                std::ffi::OsStr::new("--file-access-probe"),
                home.as_os_str(),
                std::ffi::OsStr::new(folder),
            ]);
            let (result, child) = run_worker(command, folder, deadline).await;
            folders.push(result);
            unreaped = child;
        }
        let _ = sender.send(report(folders));
        if let Some(mut child) = unreaped {
            let _ = child.wait().await;
        }
    });
    receiver
        .await
        .map_err(|_| "file access supervisor stopped".into())
}

async fn run_worker(
    mut command: Command,
    folder: &str,
    deadline: Duration,
) -> (FolderAccess, Option<Child>) {
    use std::process::Stdio;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return (failed(folder, "worker_start_failed"), None),
    };
    match tokio::time::timeout(deadline, child.wait()).await {
        Err(_) => {
            let _ = child.start_kill();
            let unreaped = !matches!(
                tokio::time::timeout(Duration::from_millis(250), child.wait()).await,
                Ok(Ok(_))
            );
            (failed(folder, "timeout"), unreaped.then_some(child))
        }
        Ok(Err(_)) => {
            let _ = child.start_kill();
            (failed(folder, "worker_wait_failed"), Some(child))
        }
        Ok(Ok(status)) if !status.success() => (failed(folder, "worker_failed"), None),
        Ok(Ok(_)) => {
            let mut bytes = Vec::new();
            // A trusted worker emits <1 KiB. A descendant holding the pipe must
            // not extend our deadline or allow unbounded buffering.
            let read = async {
                child
                    .stdout
                    .take()
                    .ok_or_else(|| io::Error::other("worker stdout missing"))?
                    .take(4097)
                    .read_to_end(&mut bytes)
                    .await
            };
            if !matches!(tokio::time::timeout(deadline, read).await, Ok(Ok(_)))
                || bytes.len() > 4096
            {
                return (failed(folder, "worker_reply_invalid"), None);
            }
            let value = serde_json::from_slice::<FolderAccess>(&bytes)
                .ok()
                .filter(|v| v.folder == folder);
            (
                value.unwrap_or_else(|| failed(folder, "worker_reply_invalid")),
                None,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observations_are_read_only_and_include_empty_and_missing_folders() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("Downloads")).unwrap();
        assert_eq!(observe(home.path(), "Downloads").state, AccessState::Normal);
        let entry = home.path().join("Downloads/private-name.txt");
        std::fs::write(&entry, "private content").unwrap();
        let encoded = serde_json::to_string(&observe(home.path(), "Downloads")).unwrap();
        assert!(!encoded.contains("private"));
        assert_eq!(std::fs::read_to_string(entry).unwrap(), "private content");
        assert_eq!(
            observe(home.path(), "Documents").reason.as_deref(),
            Some("directory_missing")
        );
    }

    #[test]
    fn denial_failure_and_normal_are_distinct_and_no_partial_result_is_normal() {
        assert_eq!(
            classify(Err(io::Error::from(io::ErrorKind::PermissionDenied))).0,
            AccessState::Restricted
        );
        assert_eq!(
            classify(Err(io::Error::from(io::ErrorKind::Other))).0,
            AccessState::Failed
        );
        let mut all = FOLDERS
            .map(|f| FolderAccess {
                folder: f.into(),
                state: AccessState::Normal,
                reason: None,
            })
            .to_vec();
        assert_eq!(report(all.clone()).state, AccessState::Normal);
        all[1].state = AccessState::Restricted;
        assert_eq!(report(all.clone()).state, AccessState::Restricted);
        all[2].state = AccessState::Failed;
        assert_eq!(report(all).state, AccessState::Failed);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_stuck_worker_is_killed_without_blocking_the_runtime() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec sleep 30"]);
        let start = std::time::Instant::now();
        let ((result, child), tick) = tokio::join!(
            run_worker(command, "Desktop", Duration::from_millis(100)),
            async {
                tokio::time::sleep(Duration::from_millis(30)).await;
                true
            }
        );
        assert!(tick);
        assert_eq!(result.reason.as_deref(), Some("timeout"));
        assert!(child.is_none());
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn worker_crash_and_bad_or_oversized_replies_are_failures() {
        for script in ["exit 7", "printf invalid", "printf '%05000d' 0"] {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            let (result, child) = run_worker(command, "Desktop", Duration::from_secs(1)).await;
            assert_eq!(result.state, AccessState::Failed);
            assert!(child.is_none());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn abandoned_callers_cannot_bypass_the_worker_limit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("blocked-probe");
        std::fs::write(&executable, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (exe, home) = (executable.clone(), dir.path().to_path_buf());
        let caller = tokio::spawn(check_with_deadline(exe, home, Duration::from_millis(100)));
        tokio::time::sleep(Duration::from_millis(40)).await;
        caller.abort();
        assert!(
            check_with_deadline(
                executable.clone(),
                dir.path().into(),
                Duration::from_millis(100)
            )
            .await
            .is_err()
        );
        tokio::time::sleep(Duration::from_millis(400)).await;
        // The detached supervisor completed and reaped all of its children.
        assert!(
            check_with_deadline(executable, dir.path().into(), Duration::from_millis(10))
                .await
                .is_ok()
        );
    }
}
