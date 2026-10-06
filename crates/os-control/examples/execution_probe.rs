//! E0 acceptance fixture; not an installed product or an MCP entry point.
//! Run as the service account: execution_probe <UID on Unix / WTS session on Windows>.
//! All writes go to a freshly created directory inside the selected user's home.
use pab_os_control::execution::{PreparedUser, current_identity};
use pab_terminal::TerminalSession;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const PAYLOAD: &str = "用户 空格 \\\" 引号\\";
fn worker(root: &Path, payload: &OsString) -> Result<(), Box<dyn std::error::Error>> {
    if payload != &OsString::from(PAYLOAD) {
        return Err("argument round trip failed".into());
    }
    let actual = current_identity()?;
    if std::env::var_os("PAB_IDENTITY_SERVICE_SENTINEL").is_some() {
        return Err("inherited service environment".into());
    }
    let home_key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = PathBuf::from(std::env::var_os(home_key).ok_or("missing home")?);
    if home != actual.home {
        return Err("environment HOME does not match native identity".into());
    }
    if std::env::current_dir()? != actual.home {
        return Err("unexpected worker cwd".into());
    }
    // create_dir, not create_dir_all: existing path means the fixture must stop.
    std::fs::create_dir(root)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("用户文件.txt"))?;
    file.write_all("执行身份验收\n".as_bytes())?;
    drop(file);
    #[cfg(unix)]
    let owner: Value = {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(root.join("用户文件.txt"))?;
        if format!("uid:{}", m.uid()) != actual.account_id {
            return Err("file owner mismatch".into());
        }
        json!({"uid":m.uid(),"gid":m.gid()})
    };
    #[cfg(windows)]
    let owner =
        json!({"validation":"inspect file ACL with native security API after worker completion"});
    #[cfg(windows)]
    let terminal =
        TerminalSession::start("C:\\Windows\\System32\\whoami.exe", &["/user"], 100, 30)?;
    #[cfg(unix)]
    let terminal = TerminalSession::start("/usr/bin/id", &[], 100, 30)?;
    terminal.resize(120, 40)?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let output = terminal.read(offset, 32 * 1024);
        offset = output.next_offset;
        bytes.extend(output.bytes);
        if output.ended {
            break;
        }
        if Instant::now() >= deadline {
            terminal.close()?;
            return Err("PTY timeout".into());
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    terminal.close()?;
    let pty = String::from_utf8_lossy(&bytes).to_string();
    #[cfg(windows)]
    let marker = actual.account_id.clone();
    #[cfg(unix)]
    let marker = format!("uid={}(", actual.account_id.strip_prefix("uid:").unwrap());
    if !pty.contains(&marker) {
        return Err(format!("PTY account mismatch: {pty}").into());
    }
    let report = json!({"identity":actual,"cwd":std::env::current_dir()?,"home":home,"file_owner":owner,"pty":pty,"payload":payload.to_string_lossy(),"service_environment_removed":true});
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("result.json"))?;
    file.write_all(serde_json::to_string_pretty(&report)?.as_bytes())?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "--sleep") {
        std::thread::sleep(Duration::from_secs(300));
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--worker") {
        let root = Path::new(args.get(1).ok_or("missing root")?);
        let result = worker(root, args.get(2).ok_or("missing payload")?);
        if let Err(error) = &result {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.join("error.txt"))
            {
                let _ = writeln!(file, "{error}");
            }
        }
        return result;
    }
    let selected: u32 = args
        .first()
        .ok_or("expected UID / Windows session ID")?
        .to_str()
        .ok_or("non-text selector")?
        .parse()?;
    let service_before = current_identity()?;
    #[cfg(unix)]
    let target = PreparedUser::for_uid(selected)?;
    #[cfg(windows)]
    let target = PreparedUser::for_session(selected)?;
    let mut negative_checks = vec![];
    if target
        .spawn(Path::new("relative-worker"), &[], &target.identity().home)
        .is_ok()
    {
        return Err("relative worker accepted".into());
    }
    negative_checks.push("relative_worker_rejected");
    let missing = target
        .identity()
        .home
        .join(".pab-missing-worker-7ff735d4-eecd-4ccc-a70e-ad809de971fa");
    if missing.exists() {
        return Err("negative-test path unexpectedly exists".into());
    }
    if target.spawn(&missing, &[], &target.identity().home).is_ok() {
        return Err("missing worker accepted".into());
    }
    negative_checks.push("missing_worker_rejected");
    #[cfg(unix)]
    {
        if PreparedUser::for_uid(u32::MAX).is_ok() {
            return Err("unknown account accepted".into());
        }
        negative_checks.push("unknown_account_rejected");
        if service_before.account_id == "uid:0" && target.identity().account_id != "uid:0" {
            use std::os::unix::fs::PermissionsExt;
            let private = tempfile::tempdir()?;
            std::fs::set_permissions(private.path(), std::fs::Permissions::from_mode(0o700))?;
            match target.spawn(
                &std::env::current_exe()?,
                &["--sleep".into()],
                private.path(),
            ) {
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => (),
                _ => return Err("target could enter service-only cwd".into()),
            }
            negative_checks.push("service_only_cwd_rejected_after_switch");
        }
    }
    #[cfg(windows)]
    {
        if PreparedUser::for_session(u32::MAX).is_ok() {
            return Err("unknown session accepted".into());
        }
        negative_checks.push("unknown_session_rejected");
    }
    let mut sleeping = target.spawn(
        &std::env::current_exe()?,
        &["--sleep".into()],
        &target.identity().home,
    )?;
    sleeping.terminate()?;
    if sleeping.try_wait()?.is_none() {
        return Err("terminated worker still running".into());
    }
    sleeping.terminate()?;
    negative_checks.push("worker_termination_confirmed_and_repeatable");
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let root = target.identity().home.join(format!(
        ".pab-execution-probe-{timestamp}-{}",
        std::process::id()
    ));
    if root.exists() {
        return Err("probe path already exists".into());
    }
    let mut child = target.spawn(
        &std::env::current_exe()?,
        &[
            "--worker".into(),
            root.clone().into_os_string(),
            PAYLOAD.into(),
        ],
        &target.identity().home,
    )?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let exit = loop {
        if let Some(code) = child.try_wait()? {
            break code;
        }
        if Instant::now() >= deadline {
            child.terminate()?;
            return Err("worker timeout".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if exit != 0 {
        let detail = std::fs::read_to_string(root.join("error.txt")).unwrap_or_default();
        return Err(format!(
            "user worker exited {exit}: {detail}; inspect {}",
            root.display()
        )
        .into());
    }
    let result: Value = serde_json::from_slice(&std::fs::read(root.join("result.json"))?)?;
    if serde_json::to_value(target.identity())? != result["identity"] {
        return Err("selected and observed identities differ".into());
    }
    if current_identity()? != service_before {
        return Err("parent identity changed".into());
    }
    // Files retained for independent ACL/owner checks; no changes to real documents.
    let report = json!({"status":"passed","service":service_before,"target":target.identity(),"child_pid":child.id(),"result":result,"artifact_directory":root,"negative_checks":negative_checks});
    io::stdout().write_all(serde_json::to_string_pretty(&report)?.as_bytes())?;
    Ok(())
}
