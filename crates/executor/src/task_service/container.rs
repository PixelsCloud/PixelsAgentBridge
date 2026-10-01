//! Docker mechanics and stream framing are provided by Bollard. No Docker CLI or shell.
use crate::task_store::TaskStore;
use bollard::{API_DEFAULT_VERSION, Docker, container::LogOutput, query_parameters::*};
use futures_util::StreamExt;
use pab_protocol::*;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use tokio::{
    sync::{Mutex, watch},
    time::{Duration, Instant},
};
static LOCKS: OnceLock<Mutex<HashMap<String, std::sync::Weak<Mutex<()>>>>> = OnceLock::new();
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn cut(s: &mut String, n: usize) {
    let mut end = n.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}
fn oid(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn error(e: bollard::errors::Error) -> String {
    let mut s = match e {
        bollard::errors::Error::DockerResponseServerError {
            status_code,
            message,
        } => format!("docker_api_{status_code}: {message}"),
        other => format!("docker_unavailable: {other}"),
    };
    cut(&mut s, 1024);
    s
}
fn default_endpoint() -> String {
    std::env::var("DOCKER_HOST").unwrap_or_else(|_| {
        if cfg!(windows) {
            "npipe:////./pipe/docker_engine".into()
        } else {
            "unix:///var/run/docker.sock".into()
        }
    })
}
fn local_client(endpoint: &str) -> Result<Docker, String> {
    #[cfg(windows)]
    let valid = endpoint
        .strip_prefix("npipe:////./pipe/")
        .is_some_and(|name| {
            !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != ".."
        });
    #[cfg(unix)]
    let valid = endpoint.starts_with("unix:///") && endpoint.len() > 8;
    #[cfg(not(any(windows, unix)))]
    let valid = false;
    if !valid || endpoint.len() > 4096 || endpoint.chars().any(char::is_control) {
        return Err("docker_endpoint_unsupported: configure a local DOCKER_HOST Unix socket/Windows named pipe for the Executor identity".into());
    }
    Docker::connect_with_local(endpoint, 300, API_DEFAULT_VERSION).map_err(error)
}
async fn resource_lock(key: String) -> Arc<Mutex<()>> {
    let mut map = LOCKS.get_or_init(Mutex::default).lock().await;
    map.retain(|_, w| w.strong_count() > 0);
    if let Some(lock) = map.get(&key).and_then(std::sync::Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    map.insert(key, Arc::downgrade(&lock));
    lock
}
struct Runner {
    docker: Docker,
    r: SystemQueryReply,
    s: ContainerSnapshot,
    store: Option<TaskStore>,
}
impl Runner {
    fn truncated(&mut self, reason: &str) {
        self.r.truncated = true;
        self.r.stop_reason = Some(reason.into());
    }
    fn field(&mut self, v: &Value, key: &str) -> Option<String> {
        let mut s = v.get(key)?.as_str()?.to_owned();
        if s.len() > 1024 {
            cut(&mut s, 1024);
            self.truncated("field_byte_budget");
        }
        Some(s)
    }
    fn labels(&mut self, v: &Value) -> Vec<ContainerLabel> {
        let Some(map) = v.as_object() else {
            return vec![];
        };
        if map.len() > 64 {
            self.truncated("detail_entry_limit");
        }
        map.iter()
            .take(64)
            .map(|(name, value)| {
                let mut name = name.clone();
                let mut value = value.as_str().unwrap_or("").to_owned();
                if name.len() > 256 || value.len() > 512 {
                    self.truncated("field_byte_budget");
                    cut(&mut name, 256);
                    cut(&mut value, 512);
                }
                ContainerLabel { name, value }
            })
            .collect()
    }
    fn list_summary(&mut self, v: &Value) -> ContainerSummary {
        let names = v["Names"]
            .as_array()
            .map(|a| {
                if a.len() > 8 {
                    self.truncated("detail_entry_limit");
                }
                a.iter()
                    .take(8)
                    .filter_map(|s| s.as_str())
                    .map(|s| {
                        let mut s = s.to_owned();
                        if s.len() > 256 {
                            cut(&mut s, 256);
                            self.truncated("field_byte_budget");
                        }
                        s
                    })
                    .collect()
            })
            .unwrap_or_default();
        let ports = v["Ports"]
            .as_array()
            .map(|a| {
                if a.len() > 32 {
                    self.truncated("detail_entry_limit");
                }
                a.iter()
                    .take(32)
                    .map(|p| ContainerPort {
                        ip: self.field(p, "IP"),
                        private_port: p["PrivatePort"]
                            .as_u64()
                            .and_then(|p| u16::try_from(p).ok()),
                        public_port: p["PublicPort"].as_u64().and_then(|p| u16::try_from(p).ok()),
                        protocol: self.field(p, "Type"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        ContainerSummary {
            id: self.field(v, "Id").unwrap_or_default(),
            names,
            image: self.field(v, "Image"),
            image_id: self.field(v, "ImageID"),
            state: self.field(v, "State"),
            status: self.field(v, "Status"),
            created_unix_seconds: v["Created"].as_i64(),
            ports,
            labels: self.labels(&v["Labels"]),
        }
    }
    fn detail(&mut self, v: &Value) -> Result<ContainerInfo, String> {
        let id = v["Id"]
            .as_str()
            .filter(|s| oid(s))
            .ok_or("docker_invalid_reply: full container ID missing")?
            .to_owned();
        let state = &v["State"];
        if !state["Running"].is_boolean() {
            return Err("docker_invalid_reply: running state missing".into());
        }
        for key in ["StartedAt", "FinishedAt"] {
            if state[key]
                .as_str()
                .is_some_and(|s| s.len() > 256 || s.chars().any(char::is_control))
            {
                return Err(
                    "docker_invalid_reply: lifecycle timestamp cannot be represented safely".into(),
                );
            }
        }
        let state = ContainerState {
            status: self.field(state, "Status"),
            running: state["Running"].as_bool().unwrap_or(false),
            paused: state["Paused"].as_bool().unwrap_or(false),
            restarting: state["Restarting"].as_bool().unwrap_or(false),
            dead: state["Dead"].as_bool().unwrap_or(false),
            pid: state["Pid"].as_i64(),
            exit_code: state["ExitCode"].as_i64(),
            oom_killed: state["OOMKilled"].as_bool().unwrap_or(false),
            error: self.field(state, "Error"),
            started_at: self.field(state, "StartedAt"),
            finished_at: self.field(state, "FinishedAt"),
            health: self.field(&state["Health"], "Status"),
        };
        if state.status.is_none() {
            return Err("docker_invalid_reply: container state missing".into());
        }
        let mut ports = vec![];
        if let Some(map) = v["NetworkSettings"]["Ports"].as_object() {
            for (key, bindings) in map {
                let (port, protocol) = key.split_once('/').unwrap_or((key.as_str(), ""));
                let port = port.parse().ok();
                let items = bindings.as_array();
                if let Some(items) = items.filter(|a| !a.is_empty()) {
                    for item in items {
                        ports.push(ContainerPort {
                            private_port: port,
                            public_port: item["HostPort"].as_str().and_then(|p| p.parse().ok()),
                            ip: self.field(item, "HostIp"),
                            protocol: Some(protocol.into()),
                        });
                        if ports.len() > 32 {
                            break;
                        }
                    }
                } else {
                    ports.push(ContainerPort {
                        private_port: port,
                        public_port: None,
                        ip: None,
                        protocol: Some(protocol.into()),
                    });
                }
                if ports.len() > 32 {
                    self.truncated("detail_entry_limit");
                    ports.truncate(32);
                    break;
                }
            }
        }
        let mounts = v["Mounts"]
            .as_array()
            .map(|a| {
                if a.len() > 32 {
                    self.truncated("detail_entry_limit");
                }
                a.iter()
                    .take(32)
                    .map(|m| ContainerMount {
                        kind: self.field(m, "Type"),
                        source: self.field(m, "Source"),
                        destination: self.field(m, "Destination"),
                        read_write: m["RW"].as_bool().unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let networks = v["NetworkSettings"]["Networks"]
            .as_object()
            .map(|map| {
                if map.len() > 16 {
                    self.truncated("detail_entry_limit");
                }
                map.iter()
                    .take(16)
                    .map(|(name, n)| {
                        let mut name = name.clone();
                        if name.len() > 256 {
                            self.truncated("field_byte_budget");
                        }
                        cut(&mut name, 256);
                        ContainerNetwork {
                            name,
                            ip_address: self.field(n, "IPAddress"),
                            global_ipv6_address: self.field(n, "GlobalIPv6Address"),
                            gateway: self.field(n, "Gateway"),
                            mac_address: self.field(n, "MacAddress"),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(ContainerInfo {
            id,
            name: self.field(v, "Name"),
            image: self.field(&v["Config"], "Image"),
            image_id: self.field(v, "Image"),
            created: self.field(v, "Created"),
            state,
            tty: v["Config"]["Tty"].as_bool().unwrap_or(false),
            log_driver: self.field(&v["HostConfig"]["LogConfig"], "Type"),
            restart_policy: self.field(&v["HostConfig"]["RestartPolicy"], "Name"),
            ports,
            mounts,
            networks,
            labels: self.labels(&v["Config"]["Labels"]),
        })
    }
    async fn inspect(&mut self, id: &str) -> Result<ContainerInfo, String> {
        let value = self
            .docker
            .inspect_container(id, None)
            .await
            .map_err(error)?;
        let detail = self.detail(&serde_json::to_value(value).map_err(|e| e.to_string())?)?;
        if oid(id) && detail.id != id {
            return Err("container_identity_mismatch: inspect returned a different full ID".into());
        }
        Ok(detail)
    }
    async fn stage(&mut self, phase: &str) -> Result<(), String> {
        self.s.phase = phase.into();
        if let Some(store) = &self.store {
            let reply = bounded(self.r.clone(), self.s.clone());
            store
                .save_system_progress(&reply)
                .await
                .map_err(|e| format!("cannot persist Docker phase: {e}"))?;
        }
        Ok(())
    }
    async fn connect(&mut self) -> Result<(), String> {
        self.stage("connecting_docker").await?;
        self.docker = self
            .docker
            .clone()
            .negotiate_version()
            .await
            .map_err(error)?;
        let version = self.docker.client_version();
        self.s.api_version = Some(format!(
            "{}.{}",
            version.major_version, version.minor_version
        ));
        // Bollard 0.21.1's URI builder joins an absolute path and drops /vX.Y.
        // Use its supported request modifier to preserve the negotiated wire version.
        self.docker = self
            .docker
            .clone()
            .with_request_modifier(move |mut request| {
                let uri = request.uri();
                if uri.path() != "/version" && !uri.path().starts_with("/v1.") {
                    let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
                    let address = format!(
                        "{}://{}/v{}.{}{}",
                        uri.scheme_str().unwrap_or("http"),
                        uri.authority().map(|a| a.as_str()).unwrap_or("localhost"),
                        version.major_version,
                        version.minor_version,
                        path
                    );
                    *request.uri_mut() = address
                        .parse()
                        .expect("valid existing Docker URI with numeric version prefix");
                }
                request
            });
        let info = self.docker.info().await.map_err(error)?;
        let environment = serde_json::json!({"os":info.os_type,"architecture":info.architecture,"version":info.server_version});
        self.s.engine_os = self.field(&environment, "os");
        self.s.engine_architecture = self.field(&environment, "architecture");
        self.s.engine_version = self.field(&environment, "version");
        self.s.engine_id = info.id.filter(|s| !s.is_empty() && s.len() <= 256);
        if self.s.engine_id.is_none() {
            return Err("docker_invalid_reply: engine identity missing".into());
        }
        Ok(())
    }
    async fn perform(&mut self, q: &ContainerQuery) -> Result<(), String> {
        self.connect().await?;
        if let ContainerAction::List {
            all,
            name,
            states,
            labels,
            limit,
        } = &q.action
        {
            self.stage("listing_containers").await?;
            let mut filters = HashMap::new();
            if !*all {
                if !states.is_empty() && !states.iter().any(|s| s == "running") {
                    self.s.phase = "completed".into();
                    self.s.observed_at_unix_ms = Some(now());
                    return Ok(());
                }
                filters.insert("status".into(), vec!["running".into()]);
            } else if !states.is_empty() {
                filters.insert("status".into(), states.clone());
            }
            if !labels.is_empty() {
                filters.insert("label".into(), labels.clone());
            }
            // A bounded scan precedes the local literal-name filter; omissions are always explicit.
            let opts = ListContainersOptions {
                all: *all,
                limit: Some(10001),
                filters: Some(filters),
                ..Default::default()
            };
            let entries = self
                .docker
                .list_containers(Some(opts))
                .await
                .map_err(error)?;
            if entries.len() > 10000 {
                self.truncated("container_scan_limit");
            }
            let pattern = name.as_ref().map(|s| s.to_lowercase());
            let mut summaries = vec![];
            for entry in entries.into_iter().take(10000) {
                let v = serde_json::to_value(entry).map_err(|e| e.to_string())?;
                let item = self.list_summary(&v);
                if !oid(&item.id) {
                    return Err("docker_invalid_reply: full container ID missing".into());
                }
                if !*all && item.state.as_deref() != Some("running") {
                    continue;
                }
                if !states.is_empty()
                    && !states
                        .iter()
                        .any(|s| Some(s.as_str()) == item.state.as_deref())
                {
                    continue;
                }
                if pattern.as_ref().is_some_and(|p| {
                    !v["Names"].as_array().is_some_and(|names| {
                        names
                            .iter()
                            .filter_map(Value::as_str)
                            .any(|n| n.to_lowercase().contains(p))
                    })
                }) {
                    continue;
                }
                summaries.push(item);
            }
            summaries.sort_by(|a, b| a.id.cmp(&b.id));
            if summaries.len() > *limit as usize {
                self.truncated("container_entry_limit");
                summaries.truncate(*limit as usize);
            }
            self.s.entries = summaries;
        } else {
            self.stage("resolving_container").await?;
            let selector = q.selector();
            let before = self.inspect(selector).await?;
            if selector != before.id
                && before.name.as_deref().map(|s| s.trim_start_matches('/'))
                    != Some(selector.trim_start_matches('/'))
            {
                return Err("container_not_found: select an exact name or full ID; abbreviated IDs are not accepted".into());
            }
            let id = before.id.clone();
            self.s.container_id = Some(id.clone());
            self.s.state_before = Some(before.state.clone());
            self.s.container = Some(before.clone());
            self.s.observed_at_unix_ms = Some(now());
            match &q.action {
                ContainerAction::Get { .. } => {}
                ContainerAction::Logs {
                    since,
                    until,
                    tail,
                    stdout,
                    stderr,
                    timestamps,
                    max_bytes,
                    ..
                } => {
                    if before.tty && !stdout {
                        return Err(
                            "docker_tty_streams_merged: TTY logs cannot select stderr separately"
                                .into(),
                        );
                    }
                    if before.tty {
                        self.r.warnings.push(
                            "TTY logs merge stdout/stderr; stream attribution is unavailable"
                                .into(),
                        );
                    }
                    self.stage("reading_logs").await?;
                    let options = LogsOptions {
                        follow: false,
                        stdout: *stdout,
                        stderr: *stderr,
                        since: since.unwrap_or(0) as i32,
                        until: until.unwrap_or(0) as i32,
                        timestamps: *timestamps,
                        tail: tail.to_string(),
                    };
                    let mut stream = self.docker.logs(&id, Some(options));
                    let mut bytes = 0usize;
                    let mut parts: [Vec<u8>; 3] = Default::default();
                    while let Some(item) = stream.next().await {
                        let item = item.map_err(error)?;
                        let (index, message) = match item {
                            LogOutput::StdOut { message } => (0, message),
                            LogOutput::StdErr { message } => (1, message),
                            LogOutput::Console { message } if before.tty => (2, message),
                            LogOutput::Console { .. } => return Err(
                                "docker_invalid_log_framing: merged output for a non-TTY container"
                                    .into(),
                            ),
                            LogOutput::StdIn { .. } => {
                                return Err("docker_invalid_reply: stdin in log stream".into());
                            }
                        };
                        let take = message
                            .len()
                            .min((*max_bytes as usize).saturating_sub(bytes));
                        parts[index].extend_from_slice(&message[..take]);
                        bytes += take;
                        if take < message.len() {
                            self.truncated("log_byte_budget");
                            break;
                        }
                    }
                    for (index, mut part) in parts.into_iter().enumerate() {
                        if part.is_empty() {
                            continue;
                        }
                        if self.r.truncated
                            && let Err(e) = std::str::from_utf8(&part)
                            && e.error_len().is_none()
                        {
                            part.truncate(e.valid_up_to());
                        }
                        let lossy = std::str::from_utf8(&part).is_err();
                        if lossy && !self.r.warnings.iter().any(|w| w.starts_with("Non-UTF")) {
                            self.r.warnings.push(
                                "Non-UTF-8 log bytes are displayed with replacement characters"
                                    .into(),
                            );
                        }
                        self.s.logs.push(ContainerLog {
                            stream: ["stdout", "stderr", "console"][index].into(),
                            text: String::from_utf8_lossy(&part).into_owned(),
                        });
                    }
                    self.r.warnings.push(
                        "Log text is grouped per stream; no cross-stream ordering guarantee".into(),
                    );
                }
                ContainerAction::Control {
                    control,
                    stop_timeout_seconds,
                    ..
                } => {
                    self.s.control = Some(*control);
                    let lock = resource_lock(format!(
                        "{}:{id}",
                        self.s.engine_id.as_deref().unwrap_or("")
                    ))
                    .await;
                    let _guard = lock.try_lock().map_err(
                        |_| "container_busy: another PAB control targets this container",
                    )?;
                    if let Some(store) = &self.store
                        && store
                            .unresolved_container_control(
                                self.s.engine_id.as_deref().unwrap_or(""),
                                &id,
                                self.r.request_id,
                            )
                            .await
                            .map_err(|e| e.to_string())?
                    {
                        return Err("container_busy_unconfirmed: query the earlier pinned operation before another control".into());
                    }
                    // Re-observe after taking the resource lock. Never resolve the name again.
                    let observed = self.inspect(&id).await?;
                    self.s.state_before = Some(observed.state.clone());
                    self.s.container = Some(observed.clone());
                    if *control != ContainerControl::Restart
                        && desired(*control, &observed.state, Some(&observed.state))
                    {
                        self.s.desired_state_observed = true;
                        self.s.phase = "already_in_desired_state".into();
                        return Ok(());
                    }
                    if *control == ContainerControl::Restart
                        && observed
                            .state
                            .started_at
                            .as_ref()
                            .is_none_or(|s| s.is_empty())
                    {
                        return Err(
                            "docker_invalid_reply: restart requires the original StartedAt".into(),
                        );
                    }
                    if *control == ContainerControl::Start
                        && (observed.state.paused || observed.state.dead)
                    {
                        return Err("container_state_conflict: start does not unpause or revive a dead container".into());
                    }
                    self.stage("submitting_control").await?;
                    self.s.submission_started = true;
                    self.stage("control_submission_started").await?;
                    let result = match control {
                        ContainerControl::Start => self.docker.start_container(&id, None).await,
                        ContainerControl::Stop => {
                            self.docker
                                .stop_container(
                                    &id,
                                    Some(StopContainerOptions {
                                        t: Some(i32::from(*stop_timeout_seconds)),
                                        ..Default::default()
                                    }),
                                )
                                .await
                        }
                        ContainerControl::Restart => {
                            self.docker
                                .restart_container(
                                    &id,
                                    Some(RestartContainerOptions {
                                        t: Some(i32::from(*stop_timeout_seconds)),
                                        ..Default::default()
                                    }),
                                )
                                .await
                        }
                    };
                    if let Err(e) = result {
                        return Err(error(e));
                    }
                    self.s.daemon_acknowledged = true;
                    self.stage("waiting_for_container_state").await?;
                    loop {
                        let observed = self.inspect(&id).await?;
                        self.s.observed_at_unix_ms = Some(now());
                        let done = desired(*control, &observed.state, self.s.state_before.as_ref());
                        self.s.container = Some(observed);
                        if done {
                            self.s.desired_state_observed = true;
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
                ContainerAction::List { .. } => unreachable!(),
            }
        }
        self.s.observed_at_unix_ms = Some(now());
        self.s.phase = "completed".into();
        Ok(())
    }
}
fn desired(
    control: ContainerControl,
    state: &ContainerState,
    before: Option<&ContainerState>,
) -> bool {
    match control {
        ContainerControl::Start => {
            state.running
                && !state.paused
                && !state.restarting
                && !state.dead
                && state.status.as_deref() == Some("running")
        }
        ContainerControl::Stop => {
            !state.running
                && !state.restarting
                && matches!(state.status.as_deref(), Some("exited" | "created"))
        }
        ContainerControl::Restart => {
            desired(ContainerControl::Start, state, None)
                && state.started_at.as_ref().is_some_and(|s| !s.is_empty())
                && before.is_some_and(|b| state.started_at != b.started_at)
        }
    }
}
fn bounded(mut r: SystemQueryReply, mut s: ContainerSnapshot) -> SystemQueryReply {
    loop {
        r.returned_count = if !s.entries.is_empty() {
            s.entries.len() as u32
        } else if !s.logs.is_empty() {
            s.logs.len() as u32
        } else {
            u32::from(s.container.is_some())
        };
        r.data = Some(SystemQueryData::Container {
            snapshot: Box::new(s.clone()),
        });
        if serde_json::to_vec(&r).is_ok_and(|v| v.len() <= MAX_SYSTEM_REPLY_BYTES) {
            return r;
        }
        r.truncated = true;
        r.stop_reason = Some("result_byte_budget".into());
        if s.entries.len() > 1 {
            s.entries.truncate(s.entries.len() / 2);
            continue;
        }
        if let Some(entry) = s.entries.first_mut() {
            if entry.labels.pop().is_some() || entry.ports.pop().is_some() {
                continue;
            }
            if entry.names.len() > 1 {
                entry.names.truncate(1);
                continue;
            }
        }
        if let Some(log) = s
            .logs
            .iter_mut()
            .max_by_key(|l| l.text.len())
            .filter(|l| !l.text.is_empty())
        {
            let length = log.text.len() / 2;
            cut(&mut log.text, length);
            continue;
        }
        if let Some(c) = &mut s.container
            && (c.labels.pop().is_some()
                || c.mounts.pop().is_some()
                || c.networks.pop().is_some()
                || c.ports.pop().is_some())
        {
            continue;
        }
        // Preserve the endpoint, full IDs, lifecycle timestamps and control evidence.
        // Escape-heavy metadata can be much larger in JSON than its UTF-8 byte count.
        let shrink = |field: &mut Option<String>| {
            if let Some(value) = field.as_mut().filter(|s| s.len() > 64) {
                let size = value.len() / 2;
                cut(value, size);
                true
            } else {
                false
            }
        };
        let mut changed = shrink(&mut r.error)
            | shrink(&mut s.engine_os)
            | shrink(&mut s.engine_architecture)
            | shrink(&mut s.engine_version);
        if let Some(entry) = s.entries.first_mut() {
            changed |=
                shrink(&mut entry.image) | shrink(&mut entry.image_id) | shrink(&mut entry.status);
        }
        if let Some(before) = &mut s.state_before {
            changed |= shrink(&mut before.error) | shrink(&mut before.health);
        }
        if let Some(c) = &mut s.container {
            changed |= shrink(&mut c.name)
                | shrink(&mut c.image)
                | shrink(&mut c.image_id)
                | shrink(&mut c.created)
                | shrink(&mut c.log_driver)
                | shrink(&mut c.restart_policy)
                | shrink(&mut c.state.error)
                | shrink(&mut c.state.health);
        }
        if changed {
            continue;
        }
        // Optional metadata must not change execution facts or erase reconciliation identity.
        let minimal = ContainerSnapshot {
            endpoint: s.endpoint,
            engine_id: s.engine_id,
            api_version: s.api_version,
            phase: s.phase,
            container_id: s.container_id,
            control: s.control,
            submission_started: s.submission_started,
            daemon_acknowledged: s.daemon_acknowledged,
            desired_state_observed: s.desired_state_observed,
            state_before: s.state_before,
            observed_at_unix_ms: s.observed_at_unix_ms,
            ..Default::default()
        };
        r.warnings
            .push("Optional Docker metadata omitted to preserve bounded identity evidence".into());
        r.warnings.truncate(8);
        for warning in &mut r.warnings {
            cut(warning, 512);
        }
        r.data = Some(SystemQueryData::Container {
            snapshot: Box::new(minimal),
        });
        r.returned_count = 0;
        return r;
    }
}
async fn execute(
    id: RequestId,
    q: &ContainerQuery,
    cancelled: &mut watch::Receiver<bool>,
    store: Option<TaskStore>,
    docker: Docker,
    endpoint: String,
) -> SystemQueryReply {
    let mut runner = Runner {
        docker,
        r: SystemQueryReply::pending(id, &SystemQuery::Container { query: q.clone() }),
        s: ContainerSnapshot {
            endpoint,
            ..Default::default()
        },
        store,
    };
    runner.r.sampled_from_unix_ms = Some(now());
    let deadline = Instant::now() + Duration::from_millis(u64::from(q.timeout_ms));
    let result = if let Err(e) = q.validate() {
        Err(e.to_owned())
    } else if *cancelled.borrow() {
        Err("cancelled_before_submission".into())
    } else {
        tokio::select! {result=runner.perform(q)=>result,_=tokio::time::sleep_until(deadline)=>Err("docker_deadline: accepted actions may continue in Docker; no rollback".into()),_=cancelled.changed()=>Err("docker_cancelled: stopped waiting; accepted actions may continue in Docker".into())}
    };
    runner.r.sampled_at_unix_ms = Some(now());
    runner.r.state = match &result {
        Ok(_) => "completed",
        Err(e) if runner.s.submission_started && !e.starts_with("docker_api_4") => "unconfirmed",
        Err(e) if e.starts_with("docker_cancelled") || e == "cancelled_before_submission" => {
            "cancelled"
        }
        Err(_) => "failed",
    }
    .into();
    runner.r.error = result.err();
    bounded(runner.r, runner.s)
}
pub(super) async fn query(
    id: RequestId,
    q: &ContainerQuery,
    mut cancelled: watch::Receiver<bool>,
    store: Option<TaskStore>,
) -> SystemQueryReply {
    let endpoint = default_endpoint();
    match local_client(&endpoint) {
        Ok(docker) => execute(id, q, &mut cancelled, store, docker, endpoint).await,
        Err(e) => {
            let mut r = SystemQueryReply::pending(id, &SystemQuery::Container { query: q.clone() });
            r.state = "failed".into();
            r.error = Some(e);
            r
        }
    }
}
async fn reconcile_client(
    q: &ContainerQuery,
    original: &SystemQueryReply,
    docker: Docker,
) -> Option<SystemQueryReply> {
    let ContainerAction::Control { control, .. } = q.action else {
        return None;
    };
    let Some(SystemQueryData::Container { snapshot }) = &original.data else {
        return None;
    };
    let id = snapshot.container_id.as_ref().filter(|s| oid(s))?;
    let engine = snapshot.engine_id.as_ref()?;
    let mut runner = Runner {
        docker,
        r: original.clone(),
        s: snapshot.as_ref().clone(),
        store: None,
    };
    runner.connect().await.ok()?;
    if runner.s.engine_id.as_ref() != Some(engine) {
        return None;
    }
    let observed = runner.inspect(id).await.ok()?;
    let matches = desired(control, &observed.state, snapshot.state_before.as_ref());
    runner.s.container = Some(observed);
    runner.s.observed_at_unix_ms = Some(now());
    runner.s.desired_state_observed = matches;
    runner.s.phase = "container_state_observed".into();
    if matches {
        runner.r.state = "completed".into();
        runner.r.error = None;
        runner.r.warnings.push("Desired state observed without resubmission; this does not attribute the change to this request".into());
    }
    Some(bounded(runner.r, runner.s))
}
pub(super) async fn reconcile(
    q: &ContainerQuery,
    original: &SystemQueryReply,
) -> Option<SystemQueryReply> {
    let Some(SystemQueryData::Container { snapshot }) = &original.data else {
        return None;
    };
    let docker = local_client(&snapshot.endpoint).ok()?;
    tokio::time::timeout(
        Duration::from_secs(3),
        reconcile_client(q, original, docker),
    )
    .await
    .ok()
    .flatten()
}
#[cfg(test)]
#[path = "container_tests.rs"]
mod tests;

#[cfg(test)]
pub(super) async fn query_with_client(
    id: RequestId,
    q: &ContainerQuery,
    mut cancelled: watch::Receiver<bool>,
    store: Option<TaskStore>,
    docker: Docker,
    endpoint: String,
) -> SystemQueryReply {
    execute(id, q, &mut cancelled, store, docker, endpoint).await
}
