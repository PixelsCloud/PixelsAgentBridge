//! Root-only, opt-in real Linux session check. Uses an isolated runtime and the
//! production user launcher/broker. Opens generated animation windows. An
//! optional explicit script exercises input or an authorized lock/login test;
//! the caller must arrange recovery for disruptive tests. Does not replace
//! installed applications or services.
#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use pab_os_control::execution::{LinuxMediaDmaBroker, LinuxMediaEnvironment, PreparedUser};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        process::Command,
        time::{Duration, Instant},
    };
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|s| s == "--media-helper") {
        let data = PathBuf::from(std::env::var_os("PAB_DATA_DIR").ok_or("missing profile")?);
        let job: Vec<String> = serde_json::from_slice(&fs::read(
            data.parent().ok_or("missing parent")?.join("job.json"),
        )?)?;
        let mut results = Vec::new();
        if job.len() == 4 {
            let output = Command::new("/usr/bin/python3")
                .arg(&job[3])
                .arg(&job[0])
                .args(["--embedded", "--uinput"])
                .output()?;
            results.push(serde_json::json!({"kind":"input", "exit":output.status.code(),
                "stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr)}));
            fs::write(
                data.join("result.json"),
                serde_json::to_vec_pretty(&results)?,
            )?;
            if !output.status.success() {
                return Err("supervised input check failed".into());
            }
        }
        // Reopen twice through the same supervised helper, then ordinary
        // PipeWire capture. Each process must close before the next begins.
        for dma in [true, true, false] {
            let mut command = Command::new("/usr/bin/python3");
            command
                .arg(&job[2])
                .args([&job[0], &job[1]])
                .arg("--embedded");
            if dma {
                command.arg("--dma-broker");
            }
            let output = command.output()?;
            results.push(serde_json::json!({"dma": dma, "exit": output.status.code(),
                "stdout": String::from_utf8_lossy(&output.stdout), "stderr": String::from_utf8_lossy(&output.stderr)}));
            fs::write(
                data.join("result.json"),
                serde_json::to_vec_pretty(&results)?,
            )?;
            if !output.status.success() {
                return Err("capture check failed".into());
            }
        }
        return Ok(());
    }
    if !matches!(args.len(), 4 | 5) || unsafe { libc::geteuid() } != 0 {
        return Err(
            "usage (root): linux_dma_broker_check DESKTOP CODEC VERIFY_SCRIPT [INPUT_SCRIPT]"
                .into(),
        );
    }
    let job: Vec<String> = args[1..]
        .iter()
        .map(|p| fs::canonicalize(p).map(|p| p.to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    let target = LinuxMediaEnvironment::observe()?;
    let before = target.clone();
    let user = PreparedUser::for_uid(target.session().uid())?;
    let profile = tempfile::Builder::new()
        .prefix("pab-dma-broker-check-")
        .tempdir_in("/run")?;
    fs::set_permissions(profile.path(), fs::Permissions::from_mode(0o711))?;
    fs::write(profile.path().join("job.json"), serde_json::to_vec(&job)?)?;
    fs::set_permissions(
        profile.path().join("job.json"),
        fs::Permissions::from_mode(0o644),
    )?;
    let data = profile.path().join("data");
    fs::create_dir(&data)?;
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700))?;
    std::os::unix::fs::chown(
        &data,
        Some(target.session().uid()),
        user.identity().primary_group,
    )?;
    let own = fs::canonicalize(std::env::current_exe()?)?;
    let mut helper = user.spawn_linux_media(&own, &target, &data, 1)?;
    let mut broker = LinuxMediaDmaBroker::new(
        &profile.path().join("dma.sock"),
        target,
        Path::new(&job[0]),
        vec!["--media-worker".into()],
        helper.id(),
    )?;
    let mut input = LinuxMediaDmaBroker::new_input(
        &profile.path().join("input.sock"),
        before.clone(),
        Path::new(&job[0]),
        vec!["--media-worker".into()],
        helper.id(),
    )?;
    let deadline = Instant::now() + Duration::from_secs(75);
    let mut errors = Vec::new();
    let exit = loop {
        if let Some(status) = helper.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline || !before.current() {
            helper.terminate()?;
            break None;
        }
        if let Err(error) = broker.poll() {
            errors.push(error.to_string());
        }
        if let Err(error) = input.poll() {
            errors.push(error.to_string());
        }
        if data.join("root-crash-request").exists() && !data.join("root-crash-done").exists() {
            // Explicit destructive test request in this isolated wrapper only.
            // Match our unreaped root input child; never touch installed apps.
            use std::os::unix::fs::MetadataExt;
            let mut candidates = Vec::new();
            for entry in fs::read_dir("/proc")? {
                let entry = entry?;
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.parse::<i32>().ok())
                else {
                    continue;
                };
                if !entry.metadata().is_ok_and(|m| m.uid() == 0) {
                    continue;
                }
                let Ok(status) = fs::read_to_string(entry.path().join("status")) else {
                    continue;
                };
                if !status
                    .lines()
                    .any(|s| s == format!("PPid:\t{}", std::process::id()))
                {
                    continue;
                }
                let Ok(bytes) = fs::read(entry.path().join("cmdline")) else {
                    continue;
                };
                let parts: Vec<_> = bytes.split(|b| *b == 0).collect();
                if parts.len() > 3
                    && parts[0] == job[0].as_bytes()
                    && parts[1] == b"--media-worker"
                    && parts[2] == b"input-session"
                {
                    candidates.push(pid);
                }
            }
            if candidates.len() != 1 {
                return Err("isolated input crash target is ambiguous".into());
            }
            if unsafe { libc::kill(candidates[0], libc::SIGKILL) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            fs::write(data.join("root-crash-done"), b"done")?;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    broker.stop()?;
    input.stop()?;
    drop(broker);
    drop(input);
    let report: serde_json::Value = serde_json::from_slice(&fs::read(data.join("result.json"))?)?;
    println!(
        "{}",
        serde_json::json!({"helper_exit": format!("{exit:?}"), "session_unchanged": before.current(),
        "broker_errors": errors, "runs": report, "socket_removed": !profile.path().join("dma.sock").exists()})
    );
    if exit != Some(0) || !errors.is_empty() || !before.current() {
        return Err("supervised DMA check failed".into());
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Linux only");
}
