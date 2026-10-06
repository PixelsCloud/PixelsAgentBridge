use crate::RequestId;
use serde::{Deserialize, Serialize};

pub const MAX_SYSTEM_REPLY_BYTES: usize = 32 * 1024;
pub const SYSTEM_QUERY_SCHEMA_VERSION: u16 = 9;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SystemQuery {
    Container {
        query: crate::ContainerQuery,
    },
    Git {
        query: crate::GitQuery,
    },
    Desktop {
        query: crate::DesktopQuery,
    },
    TerminateProcess {
        pid: u32,
        identity: String,
        timeout_ms: u32,
        force: bool,
    },
    Services {
        name: Option<String>,
        state: Option<String>,
        limit: u16,
    },
    Service {
        name: String,
    },
    ServiceControl {
        name: String,
        control: ServiceControlAction,
        timeout_ms: u32,
    },
    Connections {
        filter: ConnectionFilter,
        limit: u16,
    },
    Dns {
        name: String,
        record_type: DnsRecordType,
        timeout_ms: u32,
        limit: u16,
    },
    Sessions {
        user: Option<String>,
        state: Option<String>,
        limit: u16,
    },
    Info {
        include_gpu: bool,
        sample_cpu: bool,
    },
    Disks {
        limit: u16,
    },
    Processes {
        pid: Option<u32>,
        name: Option<String>,
        user: Option<String>,
        limit: u16,
        sample_cpu: bool,
    },
    Process {
        pid: u32,
        sample_cpu: bool,
    },
    Networks {
        name: Option<String>,
        limit: u16,
    },
}
impl SystemQuery {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Container { query } => query.kind(),
            Self::Git { query } => query.kind(),
            Self::Desktop { query } => query.kind(),
            Self::TerminateProcess { .. } => "process_terminate",
            Self::Services { .. } => "services",
            Self::Service { .. } => "service",
            Self::ServiceControl { .. } => "service_control",
            Self::Connections { .. } => "network_connections",
            Self::Dns { .. } => "dns",
            Self::Sessions { .. } => "os_sessions",
            Self::Info { .. } => "system_info",
            Self::Disks { .. } => "disks",
            Self::Processes { .. } => "processes",
            Self::Process { .. } => "process",
            Self::Networks { .. } => "network_interfaces",
        }
    }
    pub fn required_version(&self) -> u16 {
        if matches!(
            self,
            Self::Desktop {
                query: crate::DesktopQuery::Ui { .. }
            }
        ) {
            return 9;
        }
        if matches!(
            self,
            Self::Desktop {
                query: crate::DesktopQuery::MonitorInput { .. }
            }
        ) {
            return 8;
        }
        if matches!(
            self,
            Self::Desktop {
                query: crate::DesktopQuery::Batch { .. }
            }
        ) {
            return 7;
        }
        if matches!(self, Self::Container { .. }) {
            return 6;
        }
        if matches!(self, Self::Git { .. }) {
            return 5;
        }
        if matches!(self, Self::Desktop { .. }) {
            return 4;
        }
        if matches!(
            self,
            Self::TerminateProcess { .. }
                | Self::Services { .. }
                | Self::Service { .. }
                | Self::ServiceControl { .. }
        ) {
            return 3;
        }
        if matches!(
            self,
            Self::Connections { .. } | Self::Dns { .. } | Self::Sessions { .. }
        ) {
            2
        } else {
            1
        }
    }
    pub fn is_mutation(&self) -> bool {
        if let Self::Container { query } = self {
            return query.is_mutation();
        }
        if let Self::Git { query } = self {
            return query.is_mutation();
        }
        if let Self::Desktop { query } = self {
            return query.is_mutation();
        }
        matches!(
            self,
            Self::TerminateProcess { .. } | Self::ServiceControl { .. }
        )
    }
    /// Persistence-only identity; mutation text is hashed rather than stored here.
    pub fn persistence_form(&self) -> Self {
        let mut value = self.clone();
        if let Self::Desktop {
            query: crate::DesktopQuery::Ui { query },
        } = &mut value
        {
            *query = query.persistence_form();
        }
        if let Self::Git {
            query:
                crate::GitQuery {
                    action: crate::GitAction::Commit { message, .. },
                    ..
                },
        } = &mut value
        {
            *message = format!("blake3:{}", blake3::hash(message.as_bytes()));
        }
        if let Self::Desktop {
            query: crate::DesktopQuery::TypeText { text, .. },
        } = &mut value
        {
            *text = format!("blake3:{}", blake3::hash(text.as_bytes()));
        }
        if let Self::Desktop {
            query: crate::DesktopQuery::Batch { actions, .. },
        } = &mut value
        {
            for action in actions {
                if let crate::DesktopAction::TypeText { text } = action {
                    *text = format!("blake3:{}", blake3::hash(text.as_bytes()));
                }
                if let crate::DesktopAction::KeyChord { key, .. } = action {
                    *key = format!("blake3:{}", blake3::hash(key.as_bytes()));
                }
            }
        }
        value
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Self::Container { query } = self {
            return query.validate();
        }
        if let Self::Git { query } = self {
            return query.validate();
        }
        if let Self::Desktop { query } = self {
            return query.validate();
        }
        match self {
            Self::TerminateProcess {
                pid,
                identity,
                timeout_ms,
                ..
            } if *pid == 0
                || identity.is_empty()
                || identity.len() > 256
                || !identity.is_ascii()
                || identity.chars().any(char::is_control)
                || !(100..=60_000).contains(timeout_ms) =>
            {
                return Err(
                    "positive pid and exact native identity required; timeout 100..60000 ms",
                );
            }
            Self::Service { name } | Self::ServiceControl { name, .. }
                if !valid_service_name(name) =>
            {
                return Err("service name must be 1..256 UTF-8 bytes; no paths, globs or controls");
            }
            Self::ServiceControl { timeout_ms, .. } if !(100..=60_000).contains(timeout_ms) => {
                return Err("timeout 100..60000 ms");
            }
            _ => {}
        }
        if let Self::Dns {
            name,
            record_type,
            timeout_ms,
            limit,
        } = self
        {
            let is_ptr_ip =
                *record_type == DnsRecordType::Ptr && name.parse::<std::net::IpAddr>().is_ok();
            let valid_name = !name.is_empty()
                && name.len() <= 253
                && name.is_ascii()
                && !name.starts_with('.')
                && !name.contains("..")
                && name.trim_end_matches('.').split('.').all(|s| {
                    !s.is_empty()
                        && s.len() <= 63
                        && s.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                });
            if (!is_ptr_ip && !valid_name)
                || !(100..=10_000).contains(timeout_ms)
                || !(1..=100).contains(limit)
            {
                return Err(
                    "DNS name must be an ASCII domain (IDNA/punycode), or PTR IP; timeout 100..10000 ms, limit 1..100",
                );
            }
        }
        if let Self::Connections { filter, .. } = self {
            filter.validate()?;
        }
        let (limit, filters, pid) = match self {
            Self::Connections { filter, limit } => (Some(*limit), vec![], filter.pid),
            Self::Services { name, state, limit } => (Some(*limit), vec![name, state], None),
            Self::Sessions { user, state, limit } => (Some(*limit), vec![user, state], None),
            Self::Disks { limit } => (Some(*limit), vec![], None),
            Self::Processes {
                pid,
                name,
                user,
                limit,
                ..
            } => (Some(*limit), vec![name, user], *pid),
            Self::Networks { name, limit } => (Some(*limit), vec![name], None),
            Self::Process { pid, .. } => (None, vec![], Some(*pid)),
            _ => (None, vec![], None),
        };
        if limit.is_some_and(|l| !(1..=1000).contains(&l))
            || pid == Some(0)
            || filters.iter().any(|f| {
                f.as_ref().is_some_and(|s| {
                    s.is_empty() || s.len() > 256 || s.chars().any(char::is_control)
                })
            })
        {
            Err("limit must be 1..1000, pid positive, filters 1..256 UTF-8 bytes without controls")
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemQueryReply {
    pub request_id: RequestId,
    pub kind: String,
    pub state: String,
    pub sampled_from_unix_ms: Option<i64>,
    pub sampled_at_unix_ms: Option<i64>,
    pub cpu_sample_ms: Option<u64>,
    pub data: Option<SystemQueryData>,
    pub truncated: bool,
    pub stop_reason: Option<String>,
    pub returned_count: u32,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}
impl SystemQueryReply {
    pub fn pending(id: RequestId, query: &SystemQuery) -> Self {
        Self {
            request_id: id,
            kind: query.kind().into(),
            state: "running".into(),
            sampled_from_unix_ms: None,
            sampled_at_unix_ms: None,
            cpu_sample_ms: None,
            data: None,
            truncated: false,
            stop_reason: None,
            returned_count: 0,
            warnings: vec![],
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SystemQueryData {
    Container {
        snapshot: Box<crate::ContainerSnapshot>,
    },
    Git {
        snapshot: crate::GitSnapshot,
    },
    Desktop {
        snapshot: crate::DesktopSnapshot,
    },
    Services {
        backend: String,
        entries: Vec<ServiceInfo>,
    },
    Service {
        service: ServiceInfo,
    },
    ProcessTermination {
        result: ProcessTerminationResult,
    },
    ServiceControl {
        result: ServiceControlResult,
    },
    Info {
        info: Box<SystemInfo>,
    },
    Disks {
        entries: Vec<DiskInfo>,
    },
    Processes {
        entries: Vec<ProcessInfo>,
    },
    Process {
        process: ProcessInfo,
    },
    Networks {
        entries: Vec<NetworkInfo>,
    },
    Connections {
        backend: String,
        entries: Vec<ConnectionInfo>,
    },
    Dns {
        result: DnsResult,
    },
    Sessions {
        backend: String,
        entries: Vec<OsSessionInfo>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemInfo {
    pub host_name: Option<String>,
    pub os_name: Option<String>,
    pub os_version: Option<String>,
    pub kernel_version: Option<String>,
    pub architecture: String,
    pub uptime_seconds: u64,
    pub boot_time_unix_seconds: u64,
    pub cpu_brand: Option<String>,
    /// Frequency reported for the first logical CPU, in MHz; not an all-core average.
    pub cpu_frequency_mhz: Option<u64>,
    pub physical_cores: Option<u32>,
    pub logical_cores: u32,
    /// 10000 = 100%. Process CPU can exceed 10000 on multiple cores.
    pub cpu_usage_basis_points: Option<u32>,
    pub memory_total_bytes: u64,
    pub memory_available_bytes: u64,
    pub memory_used_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub executor: ProcessInfo,
    pub gpu: GpuQuery,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// Exact native creation identity, returned only by get_process. Not a permission token.
    #[serde(default)]
    pub termination_identity: Option<String>,
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub executable: Option<String>,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub status: String,
    pub start_time_unix_seconds: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub cpu_usage_basis_points: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub filesystem: String,
    pub kind: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub removable: bool,
    pub readonly: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub name: String,
    pub addresses: Vec<String>,
    pub mac: Option<String>,
    pub mtu: Option<u64>,
    pub state: String,
    pub received_bytes: u64,
    pub transmitted_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuQuery {
    pub backend: String,
    pub status: String,
    pub driver_version: Option<String>,
    pub entries: Vec<GpuInfo>,
    pub errors: Vec<String>,
    pub truncated: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuInfo {
    pub uuid: Option<String>,
    pub pci_bus_id: Option<String>,
    pub name: Option<String>,
    pub memory_total_bytes: Option<u64>,
    pub memory_used_bytes: Option<u64>,
    pub utilization_percent: Option<u32>,
    pub temperature_celsius: Option<u32>,
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_are_byte_bounded_and_processes_have_no_paging() {
        let q = SystemQuery::Processes {
            pid: None,
            name: Some("中".repeat(86)),
            user: None,
            limit: 100,
            sample_cpu: false,
        };
        assert!(q.validate().is_err());
        assert!(
            serde_json::from_value::<SystemQuery>(
                serde_json::json!({"action":"disks","limit":20,"offset":1})
            )
            .is_err()
        );
        assert!(
            SystemQuery::Process {
                pid: 0,
                sample_cpu: false
            }
            .validate()
            .is_err()
        );
        assert!(SystemQuery::Disks { limit: 1001 }.validate().is_err());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocketProtocol {
    Tcp,
    Udp,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpFamily {
    Ipv4,
    Ipv6,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ConnectionFilter {
    pub protocol: Option<SocketProtocol>,
    pub family: Option<IpFamily>,
    pub local_address: Option<std::net::IpAddr>,
    pub remote_address: Option<std::net::IpAddr>,
    pub local_port: Option<u16>,
    pub remote_port: Option<u16>,
    pub state: Option<String>,
    pub pid: Option<u32>,
}
impl ConnectionFilter {
    pub fn validate(&self) -> Result<(), &'static str> {
        const STATES: &[&str] = &[
            "closed",
            "listen",
            "syn_sent",
            "syn_received",
            "established",
            "fin_wait_1",
            "fin_wait_2",
            "close_wait",
            "closing",
            "last_ack",
            "time_wait",
            "delete_tcb",
            "unknown",
        ];
        if self.pid == Some(0)
            || self
                .state
                .as_ref()
                .is_some_and(|s| !STATES.contains(&s.as_str()))
        {
            return Err("invalid PID or TCP state");
        }
        if self.protocol == Some(SocketProtocol::Udp)
            && (self.remote_address.is_some() || self.remote_port.is_some() || self.state.is_some())
        {
            return Err("UDP has no observable remote address, port or TCP state in this backend");
        }
        for addr in [self.local_address, self.remote_address]
            .into_iter()
            .flatten()
        {
            if self
                .family
                .is_some_and(|f| (f == IpFamily::Ipv4) != addr.is_ipv4())
            {
                return Err("IP address does not match requested family");
            }
        }
        Ok(())
    }
    pub fn matches(&self, c: &ConnectionInfo) -> bool {
        self.protocol.is_none_or(|v| v == c.protocol)
            && self.family.is_none_or(|v| v == c.family)
            && self.local_address.is_none_or(|v| v == c.local_address)
            && self
                .remote_address
                .is_none_or(|v| Some(v) == c.remote_address)
            && self.local_port.is_none_or(|v| v == c.local_port)
            && self.remote_port.is_none_or(|v| Some(v) == c.remote_port)
            && self
                .state
                .as_ref()
                .is_none_or(|v| Some(v) == c.state.as_ref())
            && self.pid.is_none_or(|v| c.pids.contains(&v))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub protocol: SocketProtocol,
    pub family: IpFamily,
    pub local_address: std::net::IpAddr,
    pub local_port: u16,
    pub remote_address: Option<std::net::IpAddr>,
    pub remote_port: Option<u16>,
    pub state: Option<String>,
    pub pids: Vec<u32>,
    pub pids_truncated: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum DnsRecordType {
    A,
    Aaaa,
    Cname,
    Mx,
    Ns,
    Ptr,
    Soa,
    Srv,
    Txt,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsRecord {
    pub name: String,
    pub record_type: String,
    pub ttl_seconds: u32,
    pub value: String,
    pub value_truncated: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsResult {
    pub requested_name: String,
    pub query_name: String,
    pub record_type: DnsRecordType,
    pub resolver: String,
    pub hosts_file_consulted: bool,
    pub records: Vec<DnsRecord>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsSessionInfo {
    pub id: String,
    pub user_name: Option<String>,
    pub user_id: Option<u32>,
    pub domain: Option<String>,
    pub state: String,
    pub active: Option<bool>,
    pub remote: Option<bool>,
    pub seat: Option<String>,
    pub terminal: Option<String>,
    pub client_name: Option<String>,
    pub session_type: Option<String>,
    pub errors: Vec<String>,
}

#[cfg(test)]
mod c2_tests {
    use super::*;
    #[test]
    fn dns_validation_and_c2_capability_do_not_reject_c1_peers() {
        assert_eq!(SystemQuery::Disks { limit: 100 }.required_version(), 1);
        let make = |name: &str, kind, timeout_ms, limit| SystemQuery::Dns {
            name: name.into(),
            record_type: kind,
            timeout_ms,
            limit,
        };
        for name in [
            "example.com.",
            "_service._tcp.example.com",
            "xn--fiqs8s.example",
        ] {
            assert!(make(name, DnsRecordType::A, 5000, 20).validate().is_ok());
        }
        for ip in ["192.0.2.1", "2001:db8::1"] {
            let q = make(ip, DnsRecordType::Ptr, 5000, 20);
            assert_eq!(q.required_version(), 2);
            assert!(q.validate().is_ok());
        }
        for name in ["", "a..example", "../file", "中文.example", "a\n.example"] {
            assert!(make(name, DnsRecordType::A, 5000, 20).validate().is_err());
        }
        assert!(
            make("example.com", DnsRecordType::A, 99, 20)
                .validate()
                .is_err()
        );
        assert!(
            make("example.com", DnsRecordType::A, 10001, 20)
                .validate()
                .is_err()
        );
        assert!(
            make("example.com", DnsRecordType::A, 100, 101)
                .validate()
                .is_err()
        );
        assert_eq!(serde_json::to_value(DnsRecordType::Aaaa).unwrap(), "AAAA");
    }
    #[test]
    fn connection_filters_reject_incompatible_family_udp_peer_and_unknown_state() {
        for f in [
            ConnectionFilter {
                pid: Some(0),
                ..Default::default()
            },
            ConnectionFilter {
                protocol: Some(SocketProtocol::Udp),
                remote_port: Some(53),
                ..Default::default()
            },
            ConnectionFilter {
                family: Some(IpFamily::Ipv6),
                local_address: Some("127.0.0.1".parse().unwrap()),
                ..Default::default()
            },
            ConnectionFilter {
                state: Some("made_up".into()),
                ..Default::default()
            },
        ] {
            assert!(
                SystemQuery::Connections {
                    filter: f,
                    limit: 100
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            serde_json::from_value::<ConnectionFilter>(serde_json::json!({"local_address":"bad"}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<SystemQuery>(
                serde_json::json!({"action":"connections","filter":{},"limit":100,"offset":2})
            )
            .is_err()
        );
    }
}

fn valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && name.len() <= 256
        && !name
            .chars()
            .any(|c| c.is_control() || "/\\*?[]".contains(c))
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceControlAction {
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub backend: String,
    pub name: String,
    pub display_name: Option<String>,
    pub state: String,
    pub sub_state: Option<String>,
    pub start_mode: Option<String>,
    pub pid: Option<u32>,
    pub executable: Option<String>,
    pub account: Option<String>,
    pub exit_code: Option<i64>,
    pub checkpoint: Option<u32>,
    pub wait_hint_ms: Option<u64>,
    pub errors: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessTerminationResult {
    pub pid: u32,
    pub identity: String,
    pub outcome: String,
    pub method: String,
    pub graceful_supported: bool,
    pub forced: bool,
    pub error: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceControlResult {
    pub name: String,
    pub control: ServiceControlAction,
    pub outcome: String,
    pub phase: String,
    pub changed: bool,
    pub job_path: Option<String>,
    pub service: Option<ServiceInfo>,
    pub error: Option<String>,
}

#[cfg(test)]
mod c3_tests {
    use super::*;
    #[test]
    fn lifecycle_capability_identity_timeouts_and_exact_names_are_required() {
        let q = SystemQuery::TerminateProcess {
            pid: 1,
            identity: "windows_filetime:123".into(),
            timeout_ms: 100,
            force: false,
        };
        assert!(q.validate().is_ok());
        assert_eq!(q.required_version(), 3);
        assert!(q.is_mutation());
        for (pid, identity, timeout) in [
            (0, "token", 100),
            (1, "", 100),
            (1, "token", 99),
            (1, "token", 60001),
        ] {
            assert!(
                SystemQuery::TerminateProcess {
                    pid,
                    identity: identity.into(),
                    timeout_ms: timeout,
                    force: true
                }
                .validate()
                .is_err()
            );
        }
        for name in ["", "../foo", "a/b", "a\\b", "a*", "a?", " a", "a\n"] {
            assert!(
                SystemQuery::Service { name: name.into() }
                    .validate()
                    .is_err()
            );
        }
        assert!(
            SystemQuery::Service {
                name: "Google Chrome Elevation Service".into()
            }
            .validate()
            .is_ok()
        );
        assert!(
            SystemQuery::Service {
                name: "服务名称".into()
            }
            .validate()
            .is_ok()
        );
        assert!(
            !SystemQuery::Services {
                name: None,
                state: None,
                limit: 100
            }
            .is_mutation()
        );
        assert!(serde_json::from_str::<SystemQuery>(r#"{"action":"terminate_process","pid":123,"identity":"x","timeout_ms":100,"force":true,"tree":true}"#).is_err());
    }
}
