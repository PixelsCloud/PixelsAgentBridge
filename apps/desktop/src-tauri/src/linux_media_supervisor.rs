//! Root service lifecycle and bounded DRM descriptor export. Image conversion,
//! codecs and ordinary input run as the verified active console user/greeter;
//! application execution keeps its existing route.
use pab_os_control::execution::{
    LinuxMediaDmaBroker, LinuxMediaEnvironment, PreparedUser, UserProcess,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const RUNTIME: &str = "/run/pixels-agent-bridge-media";
#[derive(Serialize, Deserialize)]
struct Owner {
    pid: u32,
    identity: String,
    uid: u32,
}
struct Worker {
    target: LinuxMediaEnvironment,
    process: UserProcess,
    dma: Option<LinuxMediaDmaBroker>,
    _profile: tempfile::TempDir,
}

fn private_root(path: &Path, mode: u32) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(mode))?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let info = fs::symlink_metadata(path)?;
    if !info.is_dir() || info.uid() != 0 || info.mode() & 0o777 != mode {
        return Err(io::Error::other(
            "media supervisor runtime ownership is invalid",
        ));
    }
    Ok(())
}
fn lock() -> io::Result<fs::File> {
    private_root(Path::new(RUNTIME), 0o711)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(Path::new(RUNTIME).join("supervisor.lock"))?;
    let info = file.metadata()?;
    if !info.is_file() || info.uid() != 0 || info.nlink() != 1 || info.mode() & 0o077 != 0 {
        return Err(io::Error::other(
            "media supervisor lock ownership is invalid",
        ));
    }
    // SAFETY: owned open descriptor; flock is released on process death.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::other("a media supervisor is already running"));
    }
    Ok(file)
}

fn start(executable: &Path, target: LinuxMediaEnvironment) -> io::Result<Worker> {
    let user = PreparedUser::for_uid(target.session().uid())?;
    let profile = tempfile::Builder::new()
        .prefix("helper-")
        .tempdir_in(RUNTIME)?;
    let owner = Owner {
        pid: std::process::id(),
        identity: pab_os_control::process_identity(std::process::id()).map_err(io::Error::other)?,
        uid: target.session().uid(),
    };
    // Written before any child can exist. Only root can replace this anchor.
    let pending_owner = profile.path().join("owner.tmp");
    fs::write(&pending_owner, serde_json::to_vec(&owner)?)?;
    fs::set_permissions(&pending_owner, fs::Permissions::from_mode(0o600))?;
    fs::rename(pending_owner, profile.path().join("owner.json"))?;
    let data = profile.path().join("data");
    fs::create_dir(&data)?;
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700))?;
    let token = data.join("local-access.key");
    pab_executor::local_ipc::issue_local_access(&token).map_err(io::Error::other)?;
    std::os::unix::fs::chown(&token, Some(owner.uid), user.identity().primary_group)?;
    std::os::unix::fs::chown(&data, Some(owner.uid), user.identity().primary_group)?;
    fs::set_permissions(profile.path(), fs::Permissions::from_mode(0o711))?;
    let port = pab_executor::local_ipc::local_port().map_err(io::Error::other)?;
    let process = user.spawn_linux_media(executable, &target, &data, port)?;
    let dma = match LinuxMediaDmaBroker::new(
        &profile.path().join("dma.sock"),
        target.clone(),
        executable,
        vec!["--media-worker".into()],
        process.id(),
    ) {
        Ok(broker) => Some(broker),
        Err(error) => {
            tracing::warn!(%error, "Linux DMA fallback unavailable; ordinary media remains active");
            None
        }
    };
    tracing::info!(
        session = target.session().id(),
        uid = owner.uid,
        pid = process.id(),
        "Linux media helper started"
    );
    Ok(Worker {
        target,
        process,
        dma,
        _profile: profile,
    })
}
async fn stop(worker: &mut Worker) -> io::Result<()> {
    if let Some(mut broker) = worker.dma.take() {
        broker.stop()?;
    }
    if worker.process.try_wait()?.is_some() {
        return Ok(());
    }
    // UserProcess retains the unreaped child PID and its process group. Ask the
    // helper to close native pipes/release input before the bounded group kill.
    if unsafe { libc::kill(worker.process.id() as _, libc::SIGTERM) } != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    // Native input allows 3 seconds for EOF, then up to three bounded recovery
    // passes. Keep the helper alive long enough to finish that release barrier.
    let deadline = Instant::now() + Duration::from_secs(15);
    while worker.process.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            worker.process.terminate()?;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tracing::info!(
        session = worker.target.session().id(),
        "Linux media helper reaped"
    );
    Ok(())
}

fn profile_in_use(data: &Path, uid: u32) -> io::Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    let mut expected = b"PAB_DATA_DIR=".to_vec();
    expected.extend_from_slice(data.as_os_str().as_bytes());
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        if !entry.metadata().is_ok_and(|m| m.uid() == uid) {
            continue;
        }
        let mut bytes = Vec::new();
        let Ok(file) = fs::File::open(entry.path().join("environ")) else {
            continue;
        };
        file.take(131073).read_to_end(&mut bytes)?;
        if bytes.len() > 131072 {
            return Ok(true);
        }
        if bytes.split(|b| *b == 0).any(|value| value == expected) {
            return Ok(true);
        }
    }
    Ok(false)
}
fn clean_retired_profiles() -> io::Result<()> {
    for entry in fs::read_dir(RUNTIME)? {
        let entry = entry?;
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|s| s.starts_with("helper-"))
        {
            continue;
        }
        let info = fs::symlink_metadata(entry.path())?;
        if !info.is_dir() || info.uid() != 0 || info.mode() & 0o022 != 0 {
            continue;
        }
        let owner_path = entry.path().join("owner.json");
        let owner: Owner = match fs::symlink_metadata(&owner_path) {
            Ok(info) if info.is_file() && info.uid() == 0 && info.len() <= 4096 => {
                let Ok(owner) = serde_json::from_slice(&fs::read(owner_path)?) else {
                    continue;
                };
                owner
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // The sole supervisor lock is held. No owner means startup
                // ended before issuing a token or spawning a child.
                fs::remove_dir_all(entry.path())?;
                continue;
            }
            _ => continue,
        };
        if pab_os_control::process_identity(owner.pid).as_ref().ok() == Some(&owner.identity) {
            continue;
        }
        if !profile_in_use(&entry.path().join("data"), owner.uid)? {
            // The root-owned anchor cannot be renamed by the helper. Rust's
            // remove_dir_all does not follow symlinks inside its private data.
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

pub fn run() -> Result<(), String> {
    // A dedicated system service owns this entry point, never a desktop user.
    if unsafe { libc::geteuid() } != 0 {
        return Err("media supervisor requires the system service".into());
    }
    let _lock = lock().map_err(|e| e.to_string())?;
    let paths = pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::Machine)
        .map_err(|e| e.to_string())?;
    pab_logging::init("media-supervisor", paths.root()).map_err(|e| e.to_string())?;
    let executable: PathBuf = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|e| e.to_string())?;
    let info = fs::metadata(&executable).map_err(|e| e.to_string())?;
    if !info.is_file() || info.uid() != 0 || info.mode() & 0o022 != 0 {
        return Err("media supervisor executable must be owned by root".into());
    }
    tauri::async_runtime::block_on(async {
        use tokio::signal::unix::{SignalKind, signal};
        let mut termination = signal(SignalKind::terminate())?;
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut worker: Option<Worker> = None;
        let mut last_error = None;
        let mut last_cleanup = Instant::now() - Duration::from_secs(10);
        loop {
            let observed = LinuxMediaEnvironment::observe();
            let running = match worker.as_mut() {
                Some(worker) => worker.process.try_wait()?.is_none(),
                None => false,
            };
            if worker
                .as_ref()
                .is_some_and(|w| !running || observed.as_ref().ok() != Some(&w.target))
            {
                if let Some(mut old) = worker.take() {
                    stop(&mut old).await?;
                }
            }
            let result = match observed {
                Ok(target) if worker.is_none() => {
                    start(&executable, target).map(|new| worker = Some(new))
                }
                Ok(_) => Ok(()),
                Err(error) => Err(error),
            };
            if let Some(broker) = worker.as_mut().and_then(|w| w.dma.as_mut()) {
                if let Err(error) = broker.poll() {
                    tracing::debug!(%error, "Linux DMA request retired");
                }
            }
            let error = result.err().map(|e| e.to_string());
            if error != last_error {
                if let Some(error) = &error {
                    tracing::debug!(%error, "Linux console media waiting");
                }
                last_error = error;
            }
            if last_cleanup.elapsed() >= Duration::from_secs(10) {
                if let Err(error) = clean_retired_profiles() {
                    tracing::warn!(%error, "Linux media runtime cleanup deferred");
                }
                last_cleanup = Instant::now();
            }
            tokio::select! {
                _ = termination.recv() => break,
                _ = interrupt.recv() => break,
                _ = tokio::time::sleep(Duration::from_millis(500)) => {},
            }
        }
        if let Some(mut worker) = worker {
            stop(&mut worker).await?;
        }
        Ok::<(), io::Error>(())
    })
    .map_err(|e| e.to_string())
}
