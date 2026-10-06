use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{DeviceId, TenantId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceRef {
    pub tenant_id: TenantId,
    pub device_id: DeviceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OsFamily {
    Windows,
    Linux,
    Macos,
}

impl fmt::Display for OsFamily {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Windows => "Windows",
            Self::Linux => "Linux",
            Self::Macos => "macOS",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuArchitecture {
    X86_64,
    Aarch64,
}

impl fmt::Display for CpuArchitecture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionScope {
    Native,
}

impl fmt::Display for ExecutionScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("native")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathStyle {
    Windows,
    Posix,
}

impl fmt::Display for PathStyle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Windows => "Windows paths",
            Self::Posix => "POSIX paths",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpreterContext {
    pub id: String,
    pub name: String,
    pub version: String,
    pub executable_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionContext {
    pub os_family: OsFamily,
    pub os_name: String,
    pub os_version: String,
    pub architecture: CpuArchitecture,
    pub execution_scope: ExecutionScope,
    /// None means unobserved (including historical records from older versions),
    /// never an implied root/SYSTEM or interactive user identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<crate::ExecutionIdentity>,
    pub path_style: PathStyle,
    pub interpreter: Option<InterpreterContext>,
    pub cwd: Option<String>,
    pub environment_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedEnvironment {
    pub os_family: OsFamily,
    pub environment_revision: String,
}

impl ExecutionContext {
    pub fn matches_expected(&self, expected: &ExpectedEnvironment) -> bool {
        self.os_family == expected.os_family
            && self.environment_revision == expected.environment_revision
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetContextSource {
    ExecutorVerified,
    HistoricalTask,
    Cache,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextFreshness {
    Current,
    Historical,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetContext {
    pub device_ref: DeviceRef,
    pub execution: ExecutionContext,
    pub source: TargetContextSource,
    pub observed_at_unix_ms: i64,
    pub freshness: ContextFreshness,
}

impl TargetContext {
    pub fn compact_reminder(&self) -> String {
        let interpreter = self
            .execution
            .interpreter
            .as_ref()
            .map(|value| {
                format!(
                    "{} {}",
                    single_line(&value.name, 80),
                    single_line(&value.version, 40)
                )
            })
            .unwrap_or_else(|| "no implicit shell".to_owned());
        let cwd = self
            .execution
            .cwd
            .as_deref()
            .map(|value| single_line(value, 160))
            .unwrap_or_else(|| "none".to_owned());
        let revision = single_line(&self.execution.environment_revision, 80);
        let identity = self
            .execution
            .identity
            .as_ref()
            .map(|i| {
                format!(
                    "{} ({})",
                    single_line(&i.account_name, 100),
                    single_line(&i.account_id, 100)
                )
            })
            .unwrap_or_else(|| "unobserved".to_owned());
        format!(
            "Target {}/{} | {} {} | {} | {} | {} | user={} | cwd={} | env={}",
            self.device_ref.tenant_id,
            self.device_ref.device_id,
            self.execution.os_family,
            self.execution.architecture,
            self.execution.execution_scope,
            interpreter,
            self.execution.path_style,
            identity,
            cwd,
            revision,
        )
    }
}

fn single_line(value: &str, max_chars: usize) -> String {
    let mut result = String::new();
    let mut chars = value.chars();
    for character in chars.by_ref().take(max_chars) {
        match character {
            '\r' => result.push_str("\\r"),
            '\n' => result.push_str("\\n"),
            '\t' => result.push_str("\\t"),
            value if value.is_control() => result.push('�'),
            value => result.push(value),
        }
    }
    if chars.next().is_some() {
        result.push('…');
    }
    result
}
