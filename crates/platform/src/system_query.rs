use pab_protocol::*;
use std::{
    collections::HashMap,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sysinfo::{
    Disks, Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind, Users,
};

pub struct SystemCollector {
    system: System,
    disks: Disks,
    networks: Networks,
    users: Users,
    nvml: Option<nvml_wrapper::Nvml>,
}
impl Default for SystemCollector {
    fn default() -> Self {
        Self {
            system: System::new(),
            disks: Disks::new(),
            networks: Networks::new(),
            users: Users::new(),
            nvml: None,
        }
    }
}
impl SystemCollector {
    pub fn query(&mut self, id: RequestId, query: &SystemQuery) -> SystemQueryReply {
        let mut reply = SystemQueryReply::pending(id, query);
        if let Err(e) = query.validate() {
            reply.state = "failed".into();
            reply.error = Some(e.into());
            return reply;
        }
        if let SystemQuery::Connections { filter, limit } = query {
            return super::system_query_c2::connections(reply, filter, *limit);
        }
        if !sysinfo::IS_SUPPORTED_SYSTEM {
            reply.state = "failed".into();
            reply.error = Some("unsupported operating system".into());
            return reply;
        }
        let termination_identity = if let SystemQuery::Process { pid, .. } = query {
            pab_os_control::process_identity(*pid).ok()
        } else {
            None
        };
        reply.sampled_from_unix_ms = Some(now());
        let started = Instant::now();
        let processes = matches!(
            query,
            SystemQuery::Processes { .. } | SystemQuery::Process { .. } | SystemQuery::Info { .. }
        );
        let cpu = match query {
            SystemQuery::Info { sample_cpu, .. }
            | SystemQuery::Processes { sample_cpu, .. }
            | SystemQuery::Process { sample_cpu, .. } => *sample_cpu,
            _ => false,
        };
        let pid = match query {
            SystemQuery::Process { pid, .. } => Some(*pid),
            SystemQuery::Processes { pid, .. } => *pid,
            SystemQuery::Info { .. } => Some(std::process::id()),
            _ => None,
        };
        let pids = pid.map(|p| [Pid::from_u32(p)]);
        let selection = pids
            .as_ref()
            .map_or(ProcessesToUpdate::All, |p| ProcessesToUpdate::Some(p));
        let mut identities = HashMap::new();
        if processes {
            self.users.refresh();
            self.refresh_processes(selection, cpu);
            if cpu {
                for (id, p) in self.system.processes() {
                    identities.insert(*id, p.start_time());
                }
            }
        }
        if matches!(query, SystemQuery::Info { .. }) {
            self.system.refresh_memory();
            self.system.refresh_cpu_frequency();
        }
        if cpu {
            self.system.refresh_cpu_usage();
            let sample = Instant::now();
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
            self.system.refresh_cpu_usage();
            if processes {
                self.refresh_processes(selection, true);
            }
            reply.cpu_sample_ms = Some(sample.elapsed().as_millis() as u64);
        }
        reply.data = match query {
            SystemQuery::Applications { .. } | SystemQuery::Desktop { .. } => {
                reply.state = "failed".into();
                reply.error = Some("desktop query requires the interactive helper".into());
                return reply;
            }
            SystemQuery::Container { .. }
            | SystemQuery::ExecutionContexts { .. }
            | SystemQuery::Git { .. }
            | SystemQuery::TerminateProcess { .. }
            | SystemQuery::Services { .. }
            | SystemQuery::Service { .. }
            | SystemQuery::ServiceControl { .. }
            | SystemQuery::Dns { .. }
            | SystemQuery::Sessions { .. }
            | SystemQuery::Connections { .. } => {
                return failed(reply, "this query requires its dedicated collector");
            }
            SystemQuery::Info { include_gpu, .. } => {
                let Some(executor) = self.system.process(Pid::from_u32(std::process::id())) else {
                    return failed(reply, "executor process unavailable");
                };
                let executor = self.process(executor, cpu, &identities);
                Some(SystemQueryData::Info {
                    info: Box::new(SystemInfo {
                        host_name: System::host_name().map(|s| bounded(&s, 512)),
                        os_name: System::name(),
                        os_version: System::long_os_version().map(|s| bounded(&s, 512)),
                        kernel_version: System::kernel_version(),
                        architecture: System::cpu_arch(),
                        uptime_seconds: System::uptime(),
                        boot_time_unix_seconds: System::boot_time(),
                        cpu_brand: self.system.cpus().first().map(|c| bounded(c.brand(), 256)),
                        cpu_frequency_mhz: self
                            .system
                            .cpus()
                            .first()
                            .map(|c| c.frequency())
                            .filter(|f| *f > 0),
                        physical_cores: System::physical_core_count()
                            .and_then(|n| u32::try_from(n).ok()),
                        logical_cores: self.system.cpus().len() as u32,
                        cpu_usage_basis_points: cpu
                            .then(|| basis_points(self.system.global_cpu_usage()))
                            .flatten(),
                        memory_total_bytes: self.system.total_memory(),
                        memory_available_bytes: self.system.available_memory(),
                        memory_used_bytes: self.system.used_memory(),
                        swap_total_bytes: self.system.total_swap(),
                        swap_used_bytes: self.system.used_swap(),
                        executor,
                        gpu: self.gpu(*include_gpu),
                    }),
                })
            }
            SystemQuery::Process { pid, .. } => match self.system.process(Pid::from_u32(*pid)) {
                Some(p) => Some(SystemQueryData::Process {
                    process: {
                        let mut process = self.process(p, cpu, &identities);
                        if let Some(token) = termination_identity.as_ref() {
                            if pab_os_control::process_identity(*pid).as_ref().ok() != Some(token) {
                                return failed(
                                    reply,
                                    "process identity changed during collection; query again",
                                );
                            }
                        }
                        process.termination_identity = termination_identity;
                        process
                    },
                }),
                None => return failed(reply, "process not found or not observable"),
            },
            SystemQuery::Processes {
                pid,
                name,
                user,
                limit,
                ..
            } => {
                let mut ids = self.system.processes().keys().copied().collect::<Vec<_>>();
                ids.sort_unstable();
                let mut entries = Vec::new();
                let mut output_bytes = 0;
                for id in ids {
                    if pid.is_some_and(|wanted| wanted != id.as_u32()) {
                        continue;
                    }
                    let p = &self.system.processes()[&id];
                    if name.as_ref().is_some_and(|s| {
                        !p.name()
                            .to_string_lossy()
                            .to_lowercase()
                            .contains(&s.to_lowercase())
                    }) {
                        continue;
                    }
                    let item = self.process(p, cpu, &identities);
                    if user.as_ref().is_some_and(|s| {
                        item.user_name.as_deref() != Some(s) && item.user_id.as_deref() != Some(s)
                    }) {
                        continue;
                    }
                    if entries.len() >= *limit as usize {
                        reply.truncated = true;
                        reply.stop_reason = Some("entry_limit".into());
                        break;
                    }
                    if !push_bounded(&mut entries, item, &mut output_bytes, &mut reply) {
                        break;
                    }
                }
                Some(SystemQueryData::Processes { entries })
            }
            SystemQuery::Disks { limit } => {
                self.disks.refresh(true);
                let mut entries = Vec::new();
                let mut output_bytes = 0;
                for disk in self.disks.list() {
                    if entries.len() >= *limit as usize {
                        reply.truncated = true;
                        reply.stop_reason = Some("entry_limit".into());
                        break;
                    }
                    let item = DiskInfo {
                        name: bounded(&disk.name().to_string_lossy(), 512),
                        mount_point: bounded(&disk.mount_point().to_string_lossy(), 4096),
                        filesystem: bounded(&disk.file_system().to_string_lossy(), 64),
                        kind: format!("{:?}", disk.kind()).to_lowercase(),
                        total_bytes: disk.total_space(),
                        available_bytes: disk.available_space(),
                        removable: disk.is_removable(),
                        readonly: disk.is_read_only(),
                    };
                    if !push_bounded(&mut entries, item, &mut output_bytes, &mut reply) {
                        break;
                    }
                }
                Some(SystemQueryData::Disks { entries })
            }
            SystemQuery::Networks { name, limit } => {
                self.networks.refresh(true);
                let mut entries = Vec::new();
                let mut output_bytes = 0;
                for (n, network) in &self.networks {
                    if name
                        .as_ref()
                        .is_some_and(|s| !n.to_lowercase().contains(&s.to_lowercase()))
                    {
                        continue;
                    }
                    if entries.len() >= *limit as usize {
                        reply.truncated = true;
                        reply.stop_reason = Some("entry_limit".into());
                        break;
                    }
                    let mac = network.mac_address().to_string();
                    let addresses = network
                        .ip_networks()
                        .iter()
                        .take(32)
                        .map(|ip| format!("{}/{}", ip.addr, ip.prefix))
                        .collect();
                    if network.ip_networks().len() > 32 {
                        reply
                            .warnings
                            .push("interface addresses truncated to 32".into());
                    }
                    let item = NetworkInfo {
                        name: bounded(n, 512),
                        addresses,
                        mac: (mac != "00:00:00:00:00:00").then_some(mac),
                        mtu: (network.mtu() > 0).then_some(network.mtu()),
                        state: network.operational_state().to_string(),
                        received_bytes: network.total_received(),
                        transmitted_bytes: network.total_transmitted(),
                    };
                    if !push_bounded(&mut entries, item, &mut output_bytes, &mut reply) {
                        break;
                    }
                }
                Some(SystemQueryData::Networks { entries })
            }
        };
        reply.state = "completed".into();
        reply.sampled_at_unix_ms = Some(now());
        reply.warnings.push("Fields reflect OS/library visibility; null means unavailable. Collection is not an atomic OS snapshot.".into());
        if processes {
            reply.warnings.push("Process memory 0/start time 0 are reported as unavailable; names/permissions vary by OS. CPU basis points: 10000=100%, per-process values may exceed 10000.".into());
        }
        if started.elapsed() > Duration::from_secs(5) {
            reply
                .warnings
                .push("collection exceeded 5 seconds; completed OS reads were retained".into());
        }
        bound_reply(&mut reply);
        reply
    }
    fn refresh_processes(&mut self, selection: ProcessesToUpdate<'_>, cpu: bool) {
        let mut kind = ProcessRefreshKind::nothing()
            .with_memory()
            .with_user(UpdateKind::Always)
            .with_exe(UpdateKind::Always)
            .without_tasks();
        if cpu {
            kind = kind.with_cpu();
        }
        self.system
            .refresh_processes_specifics(selection, true, kind);
    }
    fn process(
        &self,
        p: &sysinfo::Process,
        cpu: bool,
        identities: &HashMap<Pid, u64>,
    ) -> ProcessInfo {
        ProcessInfo {
            termination_identity: None,
            pid: p.pid().as_u32(),
            parent_pid: p.parent().map(|p| p.as_u32()),
            name: bounded(&p.name().to_string_lossy(), 512),
            executable: p.exe().map(|p| bounded(&p.to_string_lossy(), 4096)),
            user_id: p.user_id().map(|u| u.to_string()),
            user_name: p
                .user_id()
                .and_then(|u| self.users.get_user_by_id(u))
                .map(|u| bounded(u.name(), 256)),
            status: format!("{:?}", p.status()).to_lowercase(),
            start_time_unix_seconds: (p.start_time() > 0).then_some(p.start_time()),
            memory_bytes: (p.memory() > 0).then_some(p.memory()),
            cpu_usage_basis_points: (cpu
                && p.start_time() > 0
                && identities.get(&p.pid()) == Some(&p.start_time()))
            .then(|| basis_points(p.cpu_usage()))
            .flatten(),
        }
    }
    fn gpu(&mut self, requested: bool) -> GpuQuery {
        let mut result = GpuQuery {
            backend: "nvml".into(),
            status: "not_requested".into(),
            driver_version: None,
            entries: vec![],
            errors: vec![],
            truncated: false,
        };
        if !requested {
            return result;
        }
        if !cfg!(any(target_os = "windows", target_os = "linux")) {
            result.status = "unsupported".into();
            return result;
        }
        if self.nvml.is_none() {
            match nvml_wrapper::Nvml::init() {
                Ok(n) => self.nvml = Some(n),
                Err(e) => {
                    result.status = "unavailable".into();
                    result.errors.push(bounded(&e.to_string(), 256));
                    return result;
                }
            }
        }
        let nvml = self.nvml.as_ref().unwrap();
        result.driver_version = nvml
            .sys_driver_version()
            .map_err(|e| result.errors.push(bounded(&e.to_string(), 256)))
            .ok();
        let count = match nvml.device_count() {
            Ok(n) => n,
            Err(e) => {
                result.status = "unavailable".into();
                result.errors.push(bounded(&e.to_string(), 256));
                return result;
            }
        };
        result.truncated = count > 16;
        for index in 0..count.min(16) {
            let device = match nvml.device_by_index(index) {
                Ok(d) => d,
                Err(e) => {
                    if result.errors.len() < 8 {
                        result
                            .errors
                            .push(format!("device {index}: {}", bounded(&e.to_string(), 200)));
                    }
                    continue;
                }
            };
            let mut errors = Vec::new();
            let mut read_error = |field: &str, e: nvml_wrapper::error::NvmlError| {
                errors.push(format!("{field}: {}", bounded(&e.to_string(), 160)));
            };
            let uuid = device.uuid().map_err(|e| read_error("uuid", e)).ok();
            let name = device.name().map_err(|e| read_error("name", e)).ok();
            let pci = device
                .pci_info()
                .map_err(|e| read_error("pci", e))
                .ok()
                .map(|p| p.bus_id);
            let memory = device
                .memory_info()
                .map_err(|e| read_error("memory", e))
                .ok();
            let usage = device
                .utilization_rates()
                .map_err(|e| read_error("utilization", e))
                .ok()
                .map(|u| u.gpu);
            let temperature = device
                .temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu)
                .map_err(|e| read_error("temperature", e))
                .ok();
            result.entries.push(GpuInfo {
                uuid,
                pci_bus_id: pci,
                name,
                memory_total_bytes: memory.as_ref().map(|m| m.total),
                memory_used_bytes: memory.as_ref().map(|m| m.used),
                utilization_percent: usage,
                temperature_celsius: temperature,
                errors,
            });
        }
        result.status =
            if result.errors.is_empty() && result.entries.iter().all(|e| e.errors.is_empty()) {
                "available"
            } else {
                "partial"
            }
            .into();
        result
    }
}
pub(super) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
pub(super) fn bounded(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_owned()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s[..end].to_owned()
    }
}
fn basis_points(value: f32) -> Option<u32> {
    value
        .is_finite()
        .then_some((value.max(0.) as f64 * 100.).round().min(u32::MAX as f64) as u32)
}
pub(super) fn failed(mut r: SystemQueryReply, error: &str) -> SystemQueryReply {
    r.state = "failed".into();
    r.error = Some(error.into());
    r.sampled_at_unix_ms = Some(now());
    r
}
pub fn bound_reply(reply: &mut SystemQueryReply) {
    reply.warnings.truncate(8);
    loop {
        reply.returned_count = match &reply.data {
            Some(SystemQueryData::Applications {
                snapshot: AppSnapshot::List { snapshot },
            }) => snapshot.apps.len() as u32,
            Some(SystemQueryData::ExecutionContexts { entries, .. }) => entries.len() as u32,
            Some(SystemQueryData::Services { entries, .. }) => entries.len() as u32,
            Some(SystemQueryData::Connections { entries, .. }) => entries.len() as u32,
            Some(SystemQueryData::Sessions { entries, .. }) => entries.len() as u32,
            Some(SystemQueryData::Dns { result }) => result.records.len() as u32,
            Some(SystemQueryData::Disks { entries }) => entries.len() as u32,
            Some(SystemQueryData::Processes { entries }) => entries.len() as u32,
            Some(SystemQueryData::Networks { entries }) => entries.len() as u32,
            Some(_) => 1,
            None => 0,
        };
        if serde_json::to_vec(reply).is_ok_and(|b| b.len() <= MAX_SYSTEM_REPLY_BYTES) {
            break;
        }
        reply.truncated = true;
        reply.stop_reason = Some("output_bytes_limit".into());
        let removed = match &mut reply.data {
            Some(SystemQueryData::Applications {
                snapshot: AppSnapshot::List { snapshot },
            }) => {
                snapshot.truncated = true;
                snapshot.apps.pop().is_some()
            }
            Some(SystemQueryData::ExecutionContexts { entries, .. }) => entries.pop().is_some(),
            Some(SystemQueryData::Services { entries, .. }) => entries.pop().is_some(),
            Some(SystemQueryData::Connections { entries, .. }) => entries.pop().is_some(),
            Some(SystemQueryData::Sessions { entries, .. }) => entries.pop().is_some(),
            Some(SystemQueryData::Dns { result }) => result.records.pop().is_some(),
            Some(SystemQueryData::Disks { entries }) => entries.pop().is_some(),
            Some(SystemQueryData::Processes { entries }) => entries.pop().is_some(),
            Some(SystemQueryData::Networks { entries }) => entries.pop().is_some(),
            Some(SystemQueryData::Info { info }) => {
                info.gpu.truncated = true;
                info.gpu.entries.pop().is_some()
            }
            _ => false,
        };
        if !removed {
            let application_effect = matches!(
                &reply.data,
                Some(SystemQueryData::Applications {
                    snapshot: AppSnapshot::Action { .. }
                })
            );
            reply.data = None;
            reply.state = if application_effect {
                "unconfirmed"
            } else {
                "failed"
            }
            .into();
            reply.error = Some("result exceeds output budget".into());
            break;
        }
    }
}

pub(super) fn push_bounded<T: serde::Serialize>(
    entries: &mut Vec<T>,
    item: T,
    bytes: &mut usize,
    r: &mut SystemQueryReply,
) -> bool {
    let size = serde_json::to_vec(&item).map_or(usize::MAX, |b| b.len());
    if bytes.saturating_add(size) > 24 * 1024 {
        r.truncated = true;
        r.stop_reason = Some("output_bytes_limit".into());
        return false;
    }
    *bytes += size;
    entries.push(item);
    true
}

#[cfg(test)]
#[path = "system_query_tests.rs"]
mod tests;
