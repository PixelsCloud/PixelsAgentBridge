//! launchctl is invoked with an explicit domain/label and bounded output/deadlines.
use super::super::*;
use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const LIMIT: u64 = 2 * 1024 * 1024;

fn command(args: &[&str]) -> Result<String, String> {
    command_with_timeout(args, Duration::from_secs(5))
}

fn command_with_timeout(args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = Command::new("/bin/launchctl")
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let reader = |pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.take(LIMIT + 1).read_to_end(&mut bytes).map(|_| bytes)
        })
    };
    let out = reader(Box::new(out));
    let err = reader(Box::new(err));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!(
                    "launchctl {} timed out or failed: {other:?}",
                    args.first().unwrap_or(&"unknown")
                ));
            }
        }
    };
    let out = out
        .join()
        .map_err(|_| "launchctl reader stopped")?
        .map_err(|e| e.to_string())?;
    let err = err
        .join()
        .map_err(|_| "launchctl reader stopped")?
        .map_err(|e| e.to_string())?;
    if out.len() as u64 > LIMIT || err.len() as u64 > LIMIT {
        return Err("launchctl output exceeds budget".into());
    }
    if !status?.success() {
        return Err(format!(
            "launchctl: {}",
            bounded(&String::from_utf8_lossy(&err), 512)
        ));
    }
    String::from_utf8(out).map_err(|_| "launchctl output is not UTF-8".into())
}

fn target(name: &str) -> Result<(&str, &str), String> {
    let (domain, label) = name
        .rsplit_once('/')
        .ok_or("use system:label, user:UID:label or gui:UID:label")?;
    let valid_domain = domain == "system"
        || domain.split_once('/').is_some_and(|(kind, uid)| {
            matches!(kind, "gui" | "user")
                && !uid.is_empty()
                && uid.bytes().all(|b| b.is_ascii_digit())
                && uid.parse::<u32>().is_ok()
        });
    if !valid_domain
        || label.is_empty()
        || label.len() > 240
        || label.starts_with('.')
        || !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err("invalid launchd domain/label".into());
    }
    Ok((domain, label))
}

fn parse_service(name: &str, output: &str) -> ServiceInfo {
    let mut info = empty_service("macos_launchd", &name.replace('/', ":"));
    let mut depth = 0_i32;
    for line in output.lines() {
        let line = line.trim();
        if depth == 1
            && let Some((key, value)) = line.split_once(" = ")
        {
            match key {
                "state" => {
                    info.state = bounded(value, 64);
                    info.sub_state = Some(info.state.clone());
                }
                "pid" => info.pid = value.parse().ok().filter(|p| *p > 0),
                "program" => info.executable = Some(bounded(value, 4096)),
                "last exit code" => info.exit_code = value.parse().ok(),
                _ => {}
            }
        }
        if line.ends_with('{') {
            depth += 1;
        }
        if line == "}" {
            depth -= 1;
        }
    }
    info
}

fn disabled(domain: &str, label: &str) -> Result<bool, String> {
    let output = command(&["print-disabled", domain])?;
    for line in output.lines() {
        if let Some((key, value)) = line.trim().split_once(" => ")
            && key.trim_matches('"') == label
        {
            return Ok(value == "true" || value == "disabled");
        }
    }
    Ok(false)
}

fn read(name: &str) -> Result<ServiceInfo, String> {
    let (domain, label) = target(name)?;
    let mut info = match command(&["print", name]) {
        Ok(output) => parse_service(name, &output),
        Err(error) if error.contains("Could not find service") => {
            let path = plist_path(domain, label)?;
            let mut info = empty_service("macos_launchd", &name.replace('/', ":"));
            info.state = "not_loaded".into();
            info.executable = plist::Value::from_file(path).ok().and_then(|p| {
                p.as_dictionary()?
                    .get("Program")?
                    .as_string()
                    .map(str::to_owned)
            });
            info
        }
        Err(error) => return Err(error),
    };
    match disabled(domain, label) {
        Ok(value) => info.start_mode = Some(if value { "disabled" } else { "enabled" }.into()),
        Err(e) => info.errors.push(e),
    }
    Ok(info)
}

fn list() -> Result<SystemQueryData, String> {
    // The inventory describes the calling account's bootstrap domain. Explicit get/control
    // targets can address other domains when the operating system permits access.
    let output = command(&["list"])?;
    let manager = command(&["managername"])?;
    let domain = if manager.trim() == "System" {
        "system".to_owned()
    } else if manager.trim() == "Aqua" {
        format!("gui/{}", unsafe { libc::geteuid() })
    } else {
        format!("user/{}", unsafe { libc::geteuid() })
    };
    let mut entries = Vec::new();
    for line in output.lines().skip(1) {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() != 3 {
            return Err("unexpected launchctl list format".into());
        }
        if entries.len() >= 4096 {
            return Err("launchd inventory exceeds scan budget".into());
        }
        let name = format!("{domain}/{}", parts[2]);
        target(&name)?;
        let mut info = empty_service("macos_launchd", &name.replace('/', ":"));
        info.pid = parts[0].parse::<u32>().ok().filter(|p| *p > 0);
        info.exit_code = parts[1].parse().ok();
        info.state = if info.pid.is_some() {
            "running"
        } else {
            "not_running"
        }
        .into();
        entries.push(info);
    }
    Ok(SystemQueryData::Services {
        backend: "macos_launchd".into(),
        entries,
    })
}

fn plist_path(domain: &str, label: &str) -> Result<std::path::PathBuf, String> {
    let mut roots = if domain == "system" {
        vec![std::path::PathBuf::from("/Library/LaunchDaemons")]
    } else {
        vec![std::path::PathBuf::from("/Library/LaunchAgents")]
    };
    if domain != "system" {
        let uid = domain.split_once('/').unwrap().1.parse::<u32>().unwrap();
        // Never use Executor's HOME for another user's domain.
        let mut data = unsafe { std::mem::zeroed::<libc::passwd>() };
        let mut found = std::ptr::null_mut();
        let mut buffer = vec![0u8; 65536];
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                &mut data,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if status == 0 && !found.is_null() && !data.pw_dir.is_null() {
            let home = unsafe { std::ffi::CStr::from_ptr(data.pw_dir) }.to_string_lossy();
            roots.push(std::path::PathBuf::from(home.as_ref()).join("Library/LaunchAgents"));
        }
    }
    for root in roots {
        let path = root.join(format!("{label}.plist"));
        if path.is_file() {
            let value = plist::Value::from_file(&path).map_err(|e| e.to_string())?;
            if value
                .as_dictionary()
                .and_then(|d| d.get("Label"))
                .and_then(plist::Value::as_string)
                == Some(label)
            {
                return Ok(path);
            }
        }
    }
    Err("service has no matching plist in supported LaunchAgents/LaunchDaemons directories".into())
}

pub(super) fn execute(query: &SystemQuery) -> Result<SystemQueryData, String> {
    // Protocol service names intentionally disallow paths. Use colons on the wire;
    // translate only within the macOS backend after validation by the caller.
    let mut query = query.clone();
    match &mut query {
        SystemQuery::Service { name } | SystemQuery::ServiceControl { name, .. } => {
            *name = name.replace(':', "/")
        }
        _ => {}
    }
    execute_native(&query)
}

fn execute_native(query: &SystemQuery) -> Result<SystemQueryData, String> {
    match query {
        SystemQuery::Services { .. } => list(),
        SystemQuery::Service { name } => Ok(SystemQueryData::Service {
            service: read(name)?,
        }),
        SystemQuery::ServiceControl {
            name,
            control,
            timeout_ms,
        } => {
            let (domain, label) = target(name)?;
            if label.starts_with("com.apple.") || label.starts_with("com.pixelsagentbridge.") {
                return Err("protected system/PAB service cannot be controlled".into());
            }
            let mut result = control_result(&name.replace('/', ":"), *control);
            // Resolve the configuration before stopping so a subsequent start is possible.
            let path = plist_path(domain, label)?;
            let before = read(name).ok();
            let unchanged = match control {
                ServiceControlAction::Start => before.as_ref().is_some_and(|s| s.pid.is_some()),
                ServiceControlAction::Stop => {
                    before.as_ref().is_some_and(|s| s.state == "not_loaded")
                }
                ServiceControlAction::Enable => !disabled(domain, label)?,
                ServiceControlAction::Disable => disabled(domain, label)?,
                ServiceControlAction::Restart => false,
            };
            if unchanged {
                result.phase = "observed".into();
                result.service = before;
                return Ok(SystemQueryData::ServiceControl { result });
            }
            let deadline = Instant::now() + Duration::from_millis(u64::from(*timeout_ms));
            result.phase = "submitted".into();
            let submit = |args: &[&str]| {
                command_with_timeout(args, deadline.saturating_duration_since(Instant::now()))
            };
            let submitted = (|| -> Result<(), String> {
                match control {
                    ServiceControlAction::Enable => {
                        submit(&["enable", name])?;
                    }
                    ServiceControlAction::Disable => {
                        submit(&["disable", name])?;
                    }
                    ServiceControlAction::Stop => {
                        submit(&["bootout", name])?;
                    }
                    ServiceControlAction::Restart => {
                        submit(&["kickstart", "-k", name])?;
                    }
                    ServiceControlAction::Start => {
                        if before.as_ref().is_none_or(|s| s.state == "not_loaded") {
                            submit(&[
                                "bootstrap",
                                domain,
                                path.to_str().ok_or("non-Unicode service path")?,
                            ])?;
                        }
                        submit(&["kickstart", name])?;
                    }
                }
                Ok(())
            })();
            if let Err(error) = submitted {
                result.outcome = "unconfirmed".into();
                result.error = Some(error);
                return Ok(SystemQueryData::ServiceControl { result });
            }
            loop {
                let observed = read(name);
                let matched = match control {
                    ServiceControlAction::Enable => !disabled(domain, label)?,
                    ServiceControlAction::Disable => disabled(domain, label)?,
                    // Only the specific launchctl missing-service diagnostic confirms removal.
                    ServiceControlAction::Stop => {
                        observed.as_ref().is_ok_and(|s| s.state == "not_loaded")
                    }
                    ServiceControlAction::Start => observed.as_ref().is_ok_and(|s| s.pid.is_some()),
                    ServiceControlAction::Restart => observed.as_ref().is_ok_and(|s| {
                        s.pid.is_some() && s.pid != before.as_ref().and_then(|s| s.pid)
                    }),
                };
                if matched {
                    result.changed = true;
                    result.service = observed.ok();
                    result.phase = "observed".into();
                    break;
                }
                if Instant::now() >= deadline {
                    result.outcome = "unconfirmed".into();
                    result.error =
                        Some("launchctl accepted control; requested state was not observed".into());
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(SystemQueryData::ServiceControl { result })
        }
        _ => Err("unsupported macOS lifecycle operation".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn targets_require_explicit_domains() {
        for name in [
            "system/com.example.test",
            "gui/501/com.example.test",
            "user/501/test",
        ] {
            assert!(target(name).is_ok());
        }
        for name in [
            "test",
            "system/../test",
            "gui/-1/test",
            "system/-test;sh",
            "system/",
        ] {
            assert!(target(name).is_err());
        }
    }
    #[test]
    fn nested_environment_cannot_override_fields() {
        let info = parse_service(
            "system/test",
            "system/test = {\n state = running\n pid = 123\n environment = {\n pid = 999\n }\n}",
        );
        assert_eq!(info.pid, Some(123));
        assert_eq!(info.state, "running");
    }
    #[test]
    fn inventory_is_read_only_and_native() {
        assert!(matches!(list().unwrap(), SystemQueryData::Services { .. }));
    }

    #[test]
    fn wire_names_pass_existing_protocol_validation() {
        SystemQuery::Service {
            name: "gui:501:com.example.test".into(),
        }
        .validate()
        .unwrap();
    }

    #[test]
    #[ignore = "creates and removes a uniquely named launchd test service for this user"]
    fn temporary_launch_agent_lifecycle() {
        use std::path::PathBuf;
        let uid = unsafe { libc::geteuid() };
        assert_ne!(uid, 0, "run as a desktop user");
        let label = format!("com.pab.test.{}", RequestId::new());
        let target = format!("gui/{uid}/{label}");
        let wire = target.replace('/', ":");
        let root = PathBuf::from(std::env::var_os("HOME").unwrap()).join("Library/LaunchAgents");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!("{label}.plist"));
        struct Cleanup {
            target: String,
            path: PathBuf,
        }
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = command(&["bootout", &self.target]);
                let _ = command(&["enable", &self.target]);
                let _ = std::fs::remove_file(&self.path);
            }
        }
        let _cleanup = Cleanup {
            target: target.clone(),
            path: path.clone(),
        };
        let mut dict = plist::Dictionary::new();
        dict.insert("Label".into(), plist::Value::String(label));
        dict.insert(
            "ProgramArguments".into(),
            plist::Value::Array(vec![
                plist::Value::String("/bin/sleep".into()),
                plist::Value::String("300".into()),
            ]),
        );
        plist::Value::Dictionary(dict).to_file_xml(&path).unwrap();
        for control in [
            ServiceControlAction::Start,
            ServiceControlAction::Restart,
            ServiceControlAction::Disable,
            ServiceControlAction::Enable,
            ServiceControlAction::Stop,
        ] {
            let query = SystemQuery::ServiceControl {
                name: wire.clone(),
                control,
                timeout_ms: 20000,
            };
            query.validate().unwrap();
            let SystemQueryData::ServiceControl { result } = execute(&query).unwrap() else {
                panic!()
            };
            assert_eq!(result.outcome, "completed", "{result:?}");
        }
        let SystemQueryData::Service { service } =
            execute(&SystemQuery::Service { name: wire }).unwrap()
        else {
            panic!()
        };
        assert_eq!(service.state, "not_loaded");
    }
}
