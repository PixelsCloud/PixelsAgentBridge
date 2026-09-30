use crate::RequestId;
use serde::{Deserialize, Serialize};

pub const MAX_SYSTEM_REPLY_BYTES: usize = 32 * 1024;
pub const SYSTEM_QUERY_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SystemQuery {
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
            Self::Info { .. } => "system_info",
            Self::Disks { .. } => "disks",
            Self::Processes { .. } => "processes",
            Self::Process { .. } => "process",
            Self::Networks { .. } => "network_interfaces",
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        let (limit, filters, pid) = match self {
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
    Info { info: Box<SystemInfo> },
    Disks { entries: Vec<DiskInfo> },
    Processes { entries: Vec<ProcessInfo> },
    Process { process: ProcessInfo },
    Networks { entries: Vec<NetworkInfo> },
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
