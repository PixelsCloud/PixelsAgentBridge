use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerQuery {
    pub action: ContainerAction,
    pub timeout_ms: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContainerAction {
    List {
        all: bool,
        name: Option<String>,
        states: Vec<String>,
        labels: Vec<String>,
        limit: u16,
    },
    Get {
        container: String,
    },
    Logs {
        container: String,
        since: Option<i64>,
        until: Option<i64>,
        tail: u16,
        stdout: bool,
        stderr: bool,
        timestamps: bool,
        max_bytes: u32,
    },
    Control {
        container: String,
        control: ContainerControl,
        stop_timeout_seconds: u16,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerControl {
    Start,
    Stop,
    Restart,
}
impl ContainerQuery {
    pub fn kind(&self) -> &'static str {
        match self.action {
            ContainerAction::List { .. } => "containers",
            ContainerAction::Get { .. } => "container",
            ContainerAction::Logs { .. } => "container_logs",
            ContainerAction::Control { .. } => "container_control",
        }
    }
    pub fn is_mutation(&self) -> bool {
        matches!(self.action, ContainerAction::Control { .. })
    }
    pub fn selector(&self) -> &str {
        match &self.action {
            ContainerAction::List { .. } => "",
            ContainerAction::Get { container }
            | ContainerAction::Logs { container, .. }
            | ContainerAction::Control { container, .. } => container,
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(100..=300000).contains(&self.timeout_ms) {
            return Err("timeout_ms 100..300000");
        }
        let valid =
            |s: &str, max| !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control);
        let selector = self.selector();
        if !selector.is_empty()
            && (!valid(selector, 256)
                || !selector
                    .trim_start_matches('/')
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                || selector.trim_start_matches('/').is_empty()
                || selector.starts_with("//"))
        {
            return Err("container must be an exact name or full 64-character ID");
        }
        if !matches!(self.action, ContainerAction::List { .. }) && selector.is_empty() {
            return Err("container is required");
        }
        match &self.action {
            ContainerAction::List {
                name,
                states,
                labels,
                limit,
                ..
            } if !(1..=1000).contains(limit)
                || name.as_ref().is_some_and(|s| !valid(s, 256))
                || states.len() > 7
                || states.iter().any(|s| {
                    ![
                        "created",
                        "restarting",
                        "running",
                        "removing",
                        "paused",
                        "exited",
                        "dead",
                    ]
                    .contains(&s.as_str())
                })
                || labels.len() > 32
                || labels.iter().any(|s| !valid(s, 256)) =>
            {
                return Err("invalid container filters or limit 1..1000");
            }
            ContainerAction::Logs {
                since,
                until,
                tail,
                stdout,
                stderr,
                max_bytes,
                ..
            } if !(1..=1000).contains(tail)
                || !(1024..=16384).contains(max_bytes)
                || (!stdout && !stderr)
                || since.is_some_and(|s| !(0..=i32::MAX as i64).contains(&s))
                || until.is_some_and(|s| !(1..=i32::MAX as i64).contains(&s))
                || since.zip(*until).is_some_and(|(a, b)| a > b) =>
            {
                return Err(
                    "logs require a stream, tail 1..1000, max_bytes 1024..16384, ordered supported epoch seconds",
                );
            }
            ContainerAction::Control {
                stop_timeout_seconds,
                ..
            } if *stop_timeout_seconds > 120 => {
                return Err("stop_timeout_seconds 0..120");
            }
            _ => {}
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerSnapshot {
    pub endpoint: String,
    pub engine_id: Option<String>,
    pub engine_os: Option<String>,
    pub engine_architecture: Option<String>,
    pub engine_version: Option<String>,
    pub api_version: Option<String>,
    pub phase: String,
    pub container_id: Option<String>,
    pub control: Option<ContainerControl>,
    pub submission_started: bool,
    pub daemon_acknowledged: bool,
    pub desired_state_observed: bool,
    pub state_before: Option<ContainerState>,
    pub observed_at_unix_ms: Option<i64>,
    pub container: Option<ContainerInfo>,
    pub entries: Vec<ContainerSummary>,
    pub logs: Vec<ContainerLog>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerSummary {
    pub id: String,
    pub names: Vec<String>,
    pub image: Option<String>,
    pub image_id: Option<String>,
    pub state: Option<String>,
    pub status: Option<String>,
    pub created_unix_seconds: Option<i64>,
    pub ports: Vec<ContainerPort>,
    pub labels: Vec<ContainerLabel>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerPort {
    pub ip: Option<String>,
    pub private_port: Option<u16>,
    pub public_port: Option<u16>,
    pub protocol: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerLabel {
    pub name: String,
    pub value: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerState {
    pub status: Option<String>,
    pub running: bool,
    pub paused: bool,
    pub restarting: bool,
    pub dead: bool,
    pub pid: Option<i64>,
    pub exit_code: Option<i64>,
    pub oom_killed: bool,
    pub error: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub health: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub id: String,
    pub name: Option<String>,
    pub image: Option<String>,
    pub image_id: Option<String>,
    pub created: Option<String>,
    pub state: ContainerState,
    pub tty: bool,
    pub log_driver: Option<String>,
    pub restart_policy: Option<String>,
    pub ports: Vec<ContainerPort>,
    pub mounts: Vec<ContainerMount>,
    pub networks: Vec<ContainerNetwork>,
    pub labels: Vec<ContainerLabel>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerMount {
    pub kind: Option<String>,
    pub source: Option<String>,
    pub destination: Option<String>,
    pub read_write: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerNetwork {
    pub name: String,
    pub ip_address: Option<String>,
    pub global_ipv6_address: Option<String>,
    pub gateway: Option<String>,
    pub mac_address: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerLog {
    pub stream: String,
    pub text: String,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn container_parameters_are_strict_and_bounded() {
        let mut q = ContainerQuery {
            timeout_ms: 30000,
            action: ContainerAction::Get {
                container: "project_api-1".into(),
            },
        };
        q.validate().unwrap();
        for s in ["", "../escape", "a/b", "//bad", "bad\nname"] {
            q.action = ContainerAction::Get {
                container: s.into(),
            };
            assert!(q.validate().is_err());
        }
        q.action = ContainerAction::Logs {
            container: "api".into(),
            since: Some(2),
            until: Some(1),
            tail: 20,
            stdout: true,
            stderr: true,
            timestamps: true,
            max_bytes: 16384,
        };
        assert!(q.validate().is_err());
        assert!(serde_json::from_str::<ContainerControl>("\"kill\"").is_err());
        assert!(
            serde_json::from_str::<ContainerAction>(
                r#"{"action":"get","container":"api","endpoint":"tcp://other"}"#
            )
            .is_err()
        );
    }
}
