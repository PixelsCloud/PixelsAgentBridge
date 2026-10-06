//! Explicit non-interactive environment. Never source account shell scripts as
//! the service, nor copy its credentials or search path into another account.
use super::*;
use std::os::unix::fs::{FileTypeExt, MetadataExt};

pub(super) fn search_path(home: &Path) -> io::Result<OsString> {
    let mut paths = vec![home.join(".local/bin"), home.join(".cargo/bin")];
    if cfg!(target_os = "macos") {
        paths.push(PathBuf::from("/opt/homebrew/bin"));
        paths.push(PathBuf::from("/opt/homebrew/sbin"));
    }
    for path in [
        "/usr/local/bin",
        "/usr/local/sbin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ] {
        paths.push(PathBuf::from(path));
    }
    #[cfg(target_os = "macos")]
    {
        append_system_paths(&mut paths, Path::new("/etc/paths"));
        if let Ok(entries) = std::fs::read_dir("/etc/paths.d") {
            let mut files: Vec<_> = entries
                .take(128)
                .filter_map(Result::ok)
                .map(|e| e.path())
                .collect();
            files.sort();
            for file in files {
                append_system_paths(&mut paths, &file);
            }
        }
    }
    // A colon in a home directory cannot be represented as a PATH component.
    // Do not silently split it and accidentally search an unrelated directory.
    paths.retain(|p| !p.as_os_str().as_bytes().contains(&b':'));
    std::env::join_paths(paths).map_err(io::Error::other)
}

#[cfg(target_os = "macos")]
fn append_system_paths(paths: &mut Vec<PathBuf>, file: &Path) {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(file)
    else {
        return;
    };
    let Ok(meta) = file.metadata() else { return };
    if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o022 != 0 || meta.len() > 16384 {
        return;
    }
    let mut contents = String::new();
    if file.take(16385).read_to_string(&mut contents).is_err() || contents.len() > 16384 {
        return;
    }
    for line in contents.lines().take(256) {
        let path = PathBuf::from(line.trim());
        if path.is_absolute() && !line.contains([':', '\0']) && !paths.contains(&path) {
            paths.push(path);
        }
    }
}

fn owned_socket(path: &Path, uid: u32) -> bool {
    path.is_absolute()
        && std::fs::symlink_metadata(path)
            .is_ok_and(|m| m.file_type().is_socket() && m.uid() == uid)
}

pub(super) fn ssh_agent(uid: u32) -> Option<OsString> {
    #[cfg(target_os = "macos")]
    {
        // asuser selects the bootstrap only; it does not switch UID. Query one
        // fixed variable, then let the existing worker launcher drop identity.
        let mut query = Command::new("/bin/launchctl");
        query.args([
            "asuser",
            &uid.to_string(),
            "/bin/launchctl",
            "getenv",
            "SSH_AUTH_SOCK",
        ]);
        let bytes = bounded_query(query, std::time::Duration::from_secs(2)).ok()?;
        let value = std::str::from_utf8(&bytes).ok()?.trim_end_matches('\n');
        if value.is_empty() || value.contains(['\n', '\r', '\0']) {
            return None;
        }
        let path = Path::new(value);
        return owned_socket(path, uid).then(|| path.as_os_str().to_owned());
    }
    #[cfg(not(target_os = "macos"))]
    {
        // A headless service has no selected GUI session. Only retain an agent
        // for its own native account; never inherit root's agent for another UID.
        if unsafe { libc::geteuid() } != uid {
            return None;
        }
        let value = std::env::var_os("SSH_AUTH_SOCK")?;
        owned_socket(Path::new(&value), uid).then_some(value)
    }
}

#[cfg(any(target_os = "macos", test))]
fn bounded_query(mut command: Command, timeout: std::time::Duration) -> io::Result<Vec<u8>> {
    use std::{io::Read, os::fd::AsRawFd, time::Instant};
    command
        .env_clear()
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let child = command.spawn()?;
    let mut process = UserProcess {
        child,
        exited: false,
        group_stopped: false,
    };
    let mut output = process
        .child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing query output"))?;
    let fd = output.as_raw_fd();
    // Parent-side pipe only. A blocked/verbose launchctl cannot block a worker
    // launch indefinitely. UserProcess owns cleanup on every error path.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut status = None;
    loop {
        let mut buffer = [0u8; 4096];
        match output.read(&mut buffer) {
            Ok(0) if status.is_some() => {
                return if status == Some(0) {
                    Ok(bytes)
                } else {
                    Err(io::Error::other("user environment query failed"))
                };
            }
            Ok(n) => {
                bytes.extend_from_slice(&buffer[..n]);
                if bytes.len() > 8192 {
                    return Err(io::Error::other("user environment query too large"));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
        if status.is_none() {
            status = process.try_wait()?;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "user environment query timed out",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn environment_paths_are_absolute_and_do_not_split_account_home() {
        let home = Path::new("/home/user with spaces");
        let paths: Vec<_> = std::env::split_paths(&search_path(home).unwrap()).collect();
        assert!(paths.contains(&home.join(".local/bin")));
        assert!(paths.contains(&home.join(".cargo/bin")));
        assert!(paths.iter().all(|p| p.is_absolute()));
        let paths: Vec<_> =
            std::env::split_paths(&search_path(Path::new("/home/a:b")).unwrap()).collect();
        assert!(
            !paths
                .iter()
                .any(|p| p.starts_with("/home") || p == Path::new("b/.local/bin"))
        );
    }
    #[test]
    fn credential_socket_requires_actual_account_and_socket_type() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent");
        let _socket = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let uid = unsafe { libc::geteuid() };
        assert!(owned_socket(&path, uid));
        assert!(!owned_socket(&path, uid.wrapping_add(1)));
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(!owned_socket(&link, uid));
        assert!(!owned_socket(dir.path(), uid));
    }
    #[test]
    fn environment_query_bounds_output_and_reaps_timeout() {
        use std::time::Duration;
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "printf /fixture/agent"]);
        assert_eq!(
            bounded_query(cmd, Duration::from_secs(2)).unwrap(),
            b"/fixture/agent"
        );
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "sleep 30"]);
        assert_eq!(
            bounded_query(cmd, Duration::from_millis(50))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "yes overflow"]);
        assert!(
            bounded_query(cmd, Duration::from_secs(2))
                .unwrap_err()
                .to_string()
                .contains("too large")
        );
    }
}
