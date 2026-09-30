use super::*;
use futures_util::StreamExt;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fd::OwnedFd,
    process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal},
};
use std::time::{Duration, Instant};
use zbus_systemd::{
    systemd1::{ManagerProxy, ServiceProxy, UnitProxy},
    zbus::Connection,
};

fn fd(pid: u32) -> Result<OwnedFd, String> {
    pidfd_open(
        Pid::from_raw(i32::try_from(pid).map_err(|_| "invalid pid")?).ok_or("invalid pid")?,
        PidfdFlags::empty(),
    )
    .map_err(|e| format!("pidfd unavailable: {e}; no PID-only fallback"))
}
fn exited(fd: &OwnedFd) -> Result<bool, String> {
    let mut p = [PollFd::new(fd, PollFlags::IN)];
    poll(
        &mut p,
        Some(&Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        }),
    )
    .map_err(|e| e.to_string())?;
    Ok(p[0].revents().intersects(PollFlags::IN | PollFlags::HUP))
}
struct Lease {
    fd: OwnedFd,
    token: String,
    expires: Instant,
}
static LEASES: LazyLock<Mutex<std::collections::HashMap<u32, Lease>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
// Retaining the original pidfd eliminates same-clock-tick PID reuse ambiguity. Leases are bounded and expire explicitly.
pub(super) fn identity(pid: u32) -> Result<String, String> {
    let mut cache = LEASES
        .lock()
        .map_err(|_| "process identity cache unavailable")?;
    cache.retain(|_, lease| {
        lease.expires > Instant::now() && exited(&lease.fd).is_ok_and(|dead| !dead)
    });
    if let Some(lease) = cache.get(&pid) {
        return Ok(lease.token.clone());
    }
    if cache.len() >= 256 {
        return Err("process identity lease capacity reached; wait for lease expiry".into());
    }
    let f = fd(pid)?;
    if exited(&f)? {
        return Err("process already exited".into());
    }
    let token = format!("linux_pidfd:{}", RequestId::new());
    cache.insert(
        pid,
        Lease {
            fd: f,
            token: token.clone(),
            expires: Instant::now() + Duration::from_secs(600),
        },
    );
    Ok(token)
}
fn leased_fd(pid: u32, expected: &str) -> Result<OwnedFd, String> {
    let cache = LEASES
        .lock()
        .map_err(|_| "process identity cache unavailable")?;
    let lease = cache
        .get(&pid)
        .filter(|lease| lease.token == expected && lease.expires > Instant::now())
        .ok_or(
            "process_identity_mismatch_or_expired: query get_process again; no PID-only fallback",
        )?;
    rustix::io::dup(&lease.fd).map_err(|e| e.to_string())
}
fn wait(fd: &OwnedFd, deadline: Instant) -> Result<bool, String> {
    loop {
        if exited(fd)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
pub(super) fn terminate(query: &SystemQuery) -> Result<SystemQueryData, String> {
    let SystemQuery::TerminateProcess {
        pid,
        identity: expected,
        timeout_ms,
        force,
    } = query
    else {
        return Err("not a process termination".into());
    };
    if *pid <= 1 || *pid == std::process::id() {
        return Err("protected PID: cannot terminate Executor or init".into());
    }
    let f = leased_fd(*pid, expected)?;
    let actual = expected.clone();
    let mut r = ProcessTerminationResult {
        pid: *pid,
        identity: actual,
        outcome: "completed".into(),
        method: "already_exited".into(),
        graceful_supported: true,
        forced: false,
        error: None,
    };
    if exited(&f)? {
        return Ok(SystemQueryData::ProcessTermination { result: r });
    }
    r.method = "sigterm_pidfd".into();
    let result = (|| -> Result<(), String> {
        if let Err(e) = pidfd_send_signal(&f, Signal::TERM)
            && !exited(&f)?
        {
            return Err(e.to_string());
        }
        if wait(
            &f,
            Instant::now() + Duration::from_millis(*timeout_ms as u64),
        )? {
            return Ok(());
        }
        if !force {
            r.outcome = "timeout".into();
            return Err("SIGTERM sent but process did not exit before deadline".into());
        }
        r.forced = true;
        r.method = "sigkill_pidfd".into();
        if let Err(e) = pidfd_send_signal(&f, Signal::KILL)
            && !exited(&f)?
        {
            return Err(e.to_string());
        }
        if wait(&f, Instant::now() + Duration::from_secs(5))? {
            Ok(())
        } else {
            r.outcome = "timeout".into();
            Err("SIGKILL sent; exit not confirmed within 5 seconds".into())
        }
    })();
    if let Err(e) = result {
        if r.outcome == "completed" {
            r.outcome = "failed".into();
        }
        r.error = Some(bounded(&e, 1024));
    }
    Ok(SystemQueryData::ProcessTermination { result: r })
}
async fn read(
    connection: &Connection,
    m: &ManagerProxy<'_>,
    name: &str,
) -> Result<ServiceInfo, String> {
    let path = m.load_unit(name.into()).await.map_err(|e| e.to_string())?;
    let unit = UnitProxy::builder(connection)
        .path(path.clone())
        .map_err(|e| e.to_string())?
        .cache_properties(zbus_systemd::zbus::proxy::CacheProperties::No)
        .build()
        .await
        .map_err(|e| e.to_string())?;
    let mut r = empty_service("linux_systemd", name);
    r.state = unit.active_state().await.map_err(|e| e.to_string())?;
    macro_rules! field {
        ($field:ident,$value:expr) => {
            match $value {
                Ok(v) => r.$field = Some(bounded(&v, 4096)),
                Err(e) => r.errors.push(bounded(&e.to_string(), 256)),
            }
        };
    }
    field!(sub_state, unit.sub_state().await);
    field!(display_name, unit.description().await);
    field!(start_mode, m.get_unit_file_state(name.into()).await);
    let service = ServiceProxy::builder(connection)
        .path(path)
        .map_err(|e| e.to_string())?
        .cache_properties(zbus_systemd::zbus::proxy::CacheProperties::No)
        .build()
        .await
        .map_err(|e| e.to_string())?;
    match service.main_pid().await {
        Ok(p) => r.pid = (p > 0).then_some(p),
        Err(e) => r.errors.push(bounded(&e.to_string(), 256)),
    }
    match service.exec_main_status().await {
        Ok(e) => r.exit_code = Some(e as i64),
        Err(e) => r.errors.push(bounded(&e.to_string(), 256)),
    }
    field!(account, service.user().await);
    // Do not expose full ExecStart arguments, which may contain secrets.
    match service.exec_start().await {
        Ok(v) => r.executable = v.first().map(|e| bounded(&e.0, 4096)),
        Err(e) => r.errors.push(bounded(&e.to_string(), 256)),
    }
    r.errors.truncate(8);
    Ok(r)
}
async fn list(connection: &Connection, m: &ManagerProxy<'_>) -> Result<SystemQueryData, String> {
    let files = m.list_unit_files().await.map_err(|e| e.to_string())?;
    let units = m.list_units().await.map_err(|e| e.to_string())?;
    let mut entries = std::collections::BTreeMap::new();
    for (file, mode) in files {
        let name = file.rsplit('/').next().unwrap_or(&file);
        if name.ends_with(".service") {
            let mut r = empty_service("linux_systemd", name);
            r.state = "not_loaded".into();
            r.start_mode = Some(bounded(&mode, 128));
            entries.insert(name.to_string(), r);
        }
    }
    for (name, description, _load, active, sub, _following, _path, _job, _job_kind, _job_path) in
        units
    {
        if name.ends_with(".service") {
            let r = entries
                .entry(name.clone())
                .or_insert_with(|| empty_service("linux_systemd", &name));
            r.display_name = Some(bounded(&description, 512));
            r.state = bounded(&active, 128);
            r.sub_state = Some(bounded(&sub, 128));
        }
    }
    let _ = connection;
    Ok(SystemQueryData::Services {
        backend: "linux_systemd".into(),
        entries: entries.into_values().collect(),
    })
}
fn unit_name(name: &str) -> Result<(), String> {
    if !name.ends_with(".service") {
        Err("Linux requires an exact system service unit name ending in .service".into())
    } else {
        Ok(())
    }
}
async fn control(
    connection: &Connection,
    m: &ManagerProxy<'_>,
    name: &str,
    action: ServiceControlAction,
    timeout_ms: u32,
) -> Result<SystemQueryData, String> {
    unit_name(name)?;
    let mut r = control_result(name, action);
    let operation = async {
        r.service = Some(read(connection, m, name).await?);
        if r.service
            .as_ref()
            .is_some_and(|s| s.pid == Some(std::process::id()))
            && matches!(
                action,
                ServiceControlAction::Stop | ServiceControlAction::Restart
            )
        {
            return Err("cannot stop or restart Executor's own service".into());
        }
        if matches!(
            action,
            ServiceControlAction::Enable | ServiceControlAction::Disable
        ) {
            r.phase = "configuration_request".into();
            if action == ServiceControlAction::Enable {
                m.enable_unit_files(vec![name.into()], false, false)
                    .await
                    .map_err(|e| e.to_string())?;
            } else {
                m.disable_unit_files(vec![name.into()], false)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            r.changed = true;
            m.reload().await.map_err(|e| e.to_string())?;
            let mode = m
                .get_unit_file_state(name.into())
                .await
                .map_err(|e| e.to_string())?;
            if (action == ServiceControlAction::Enable && mode != "enabled")
                || (action == ServiceControlAction::Disable
                    && matches!(mode.as_str(), "enabled" | "enabled-runtime"))
            {
                return Err(format!(
                    "startup configuration not changed to requested state: {mode}; static/masked/runtime units may need a separate action"
                ));
            }
        } else {
            // Subscribe before dispatch so even a fast job completion cannot be missed.
            let mut signals = m.receive_job_removed().await.map_err(|e| e.to_string())?;
            m.subscribe().await.map_err(|e| e.to_string())?;
            r.phase = "job_request".into();
            let job = match action {
                ServiceControlAction::Start => m.start_unit(name.into(), "fail".into()).await,
                ServiceControlAction::Stop => m.stop_unit(name.into(), "fail".into()).await,
                ServiceControlAction::Restart => m.restart_unit(name.into(), "fail".into()).await,
                _ => unreachable!(),
            }
            .map_err(|e| e.to_string())?;
            r.changed = true;
            r.job_path = Some(job.to_string());
            r.phase = "waiting_for_job".into();
            loop {
                let signal = signals
                    .next()
                    .await
                    .ok_or("systemd job signal stream ended")?;
                let args = signal.args().map_err(|e| e.to_string())?;
                if args.job == job {
                    if args.result != "done" {
                        return Err(format!("systemd job result: {}", args.result));
                    }
                    break;
                }
            }
        }
        r.phase = "verification".into();
        let service = read(connection, m, name).await?;
        if matches!(
            action,
            ServiceControlAction::Start | ServiceControlAction::Restart
        ) && service.state != "active"
        {
            r.service = Some(service);
            return Err(
                "job finished, but service is not active (including completed oneshot services)"
                    .into(),
            );
        }
        if action == ServiceControlAction::Stop
            && !matches!(service.state.as_str(), "inactive" | "failed")
        {
            r.service = Some(service);
            return Err("job finished, but stopped state was not observed".into());
        }
        r.service = Some(service);
        Ok::<(), String>(())
    };
    match tokio::time::timeout(Duration::from_millis(timeout_ms as u64), operation).await {
        Ok(Ok(())) => r.phase = "verified".into(),
        Ok(Err(e)) => {
            r.outcome = "failed".into();
            r.error = Some(bounded(&e, 1024));
        }
        Err(_) => {
            r.outcome = "unconfirmed".into();
            r.error=Some("deadline elapsed; a submitted job/configuration change may continue. It is not cancelled or rolled back; query the service to observe current state".into());
        }
    }
    Ok(SystemQueryData::ServiceControl { result: r })
}
pub(super) async fn execute(query: &SystemQuery) -> Result<SystemQueryData, String> {
    let timeout = if let SystemQuery::ServiceControl { timeout_ms, .. } = query {
        Duration::from_millis(*timeout_ms as u64)
    } else {
        Duration::from_secs(5)
    };
    let deadline = tokio::time::Instant::now() + timeout;
    let connection = tokio::time::timeout_at(deadline, Connection::system())
        .await
        .map_err(|_| "system bus connection timed out")?
        .map_err(|e| e.to_string())?;
    let m = tokio::time::timeout_at(deadline, ManagerProxy::new(&connection))
        .await
        .map_err(|_| "systemd proxy connection timed out")?
        .map_err(|e| e.to_string())?;
    if let SystemQuery::ServiceControl {
        name,
        control: action,
        ..
    } = query
    {
        let remaining = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .as_millis() as u32;
        return control(&connection, &m, name, *action, remaining).await;
    }
    let operation = async {
        match query {
            SystemQuery::Services { .. } => list(&connection, &m).await,
            SystemQuery::Service { name } => {
                unit_name(name)?;
                Ok(SystemQueryData::Service {
                    service: read(&connection, &m, name).await?,
                })
            }
            _ => Err("unsupported service operation".into()),
        }
    };
    tokio::time::timeout_at(deadline, operation)
        .await
        .map_err(|_| "systemd collection timed out")?
}
