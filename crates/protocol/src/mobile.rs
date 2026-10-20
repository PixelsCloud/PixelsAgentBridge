//! Additive Android control contract; desktop input and task wire shapes are unchanged.
use crate::RequestId;
use serde::{Deserialize, Serialize};

pub const MOBILE_SCHEMA_VERSION: u16 = 2;
pub const MOBILE_ACTIONS: &[&str] = &[
    "observe",
    "query",
    "click",
    "long_click",
    "set_text",
    "scroll",
    "gesture",
    "navigate",
    "launch_app",
    "wait_for",
    "screenshot",
    "capabilities",
];
pub const MAX_MOBILE_ELEMENTS: usize = 128;
pub const MAX_MOBILE_OBSERVATION_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum MobileRequest {
    Execute {
        request_id: RequestId,
        command: MobileCommand,
    },
    Get {
        request_id: RequestId,
    },
    Cancel {
        request_id: RequestId,
    },
}
impl MobileRequest {
    pub fn required_version(&self) -> u16 {
        match self {
            Self::Execute { command, .. } => command.required_version(),
            _ => 1,
        }
    }
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Execute { request_id, .. }
            | Self::Get { request_id }
            | Self::Cancel { request_id } => *request_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum MobileCommand {
    Observe {},
    Capabilities {},
    Query {
        root_ref: Option<String>,
        text_contains: Option<String>,
        role: Option<String>,
        max_nodes: u16,
        max_depth: u16,
    },
    Click {
        element_ref: String,
    },
    LongClick {
        element_ref: String,
    },
    Scroll {
        element_ref: String,
        direction: MobileScrollDirection,
    },
    LaunchApp {
        package_name: String,
    },
    WaitFor {
        text: String,
        present: bool,
        timeout_ms: u64,
        interval_ms: u64,
    },
    SetText {
        element_ref: String,
        text: String,
    },
    Gesture {
        observation_ref: String,
        from_x: i32,
        from_y: i32,
        to_x: i32,
        to_y: i32,
        duration_ms: u64,
    },
    Navigate {
        destination: MobileNavigation,
    },
    Screenshot {},
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileNavigation {
    Back,
    Home,
    Recents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileScrollDirection {
    Forward,
    Backward,
    Up,
    Down,
    Left,
    Right,
}

impl MobileCommand {
    pub fn required_version(&self) -> u16 {
        match self {
            Self::Observe {}
            | Self::Click { .. }
            | Self::SetText { .. }
            | Self::Gesture { .. }
            | Self::Navigate { .. }
            | Self::Screenshot {} => 1,
            _ => 2,
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Click { element_ref }
            | Self::SetText { element_ref, .. }
            | Self::LongClick { element_ref }
            | Self::Scroll { element_ref, .. } => {
                element_ref
                    .parse::<uuid::Uuid>()
                    .map_err(|_| "invalid_element_ref")?;
            }
            Self::Query {
                root_ref,
                text_contains,
                role,
                max_nodes,
                max_depth,
            } => {
                if let Some(root) = root_ref {
                    root.parse::<uuid::Uuid>()
                        .map_err(|_| "invalid_element_ref")?;
                }
                if !(1..=128).contains(max_nodes)
                    || !(1..=32).contains(max_depth)
                    || text_contains
                        .as_ref()
                        .is_some_and(|s| s.len() > 1024 || s.contains('\0'))
                    || role
                        .as_ref()
                        .is_some_and(|s| s.len() > 256 || s.contains('\0'))
                {
                    return Err("invalid_query");
                }
            }
            Self::WaitFor {
                text,
                timeout_ms,
                interval_ms,
                ..
            } => {
                if text.is_empty()
                    || text.len() > 1024
                    || text.contains('\0')
                    || !(100..=20000).contains(timeout_ms)
                    || !(100..=1000).contains(interval_ms)
                    || interval_ms > timeout_ms
                {
                    return Err("invalid_wait");
                }
            }
            Self::LaunchApp { package_name } => {
                if package_name.len() > 255
                    || package_name.split('.').count() < 2
                    || package_name.split('.').any(|part| {
                        let mut chars = part.chars();
                        !chars
                            .next()
                            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                            || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
                    })
                {
                    return Err("invalid_package");
                }
            }
            Self::Gesture {
                observation_ref,
                duration_ms,
                from_x,
                from_y,
                to_x,
                to_y,
            } => {
                observation_ref
                    .parse::<uuid::Uuid>()
                    .map_err(|_| "invalid_observation_ref")?;
                if !(1..=2000).contains(duration_ms)
                    || [from_x, from_y, to_x, to_y]
                        .iter()
                        .any(|v| !(0..=16384).contains(*v))
                {
                    return Err("invalid_gesture");
                }
            }
            _ => {}
        }
        if let Self::SetText { text, .. } = self {
            if text.len() > 4096 || text.contains('\0') {
                return Err("invalid_text");
            }
        }
        Ok(())
    }
    pub fn is_mutation(&self) -> bool {
        !matches!(
            self,
            Self::Observe {}
                | Self::Screenshot {}
                | Self::Capabilities {}
                | Self::Query { .. }
                | Self::WaitFor { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobileBounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobileElement {
    pub element_ref: String,
    pub role: String,
    pub name: Option<String>,
    pub bounds: MobileBounds,
    pub enabled: bool,
    pub clickable: bool,
    pub editable: bool,
    pub protected: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobileObservation {
    pub observation_ref: String,
    pub revision: u64,
    pub width: u32,
    pub height: u32,
    pub rotation: u16,
    pub elements: Vec<MobileElement>,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
}
impl MobileObservation {
    pub fn validate(&self) -> bool {
        self.observation_ref.parse::<uuid::Uuid>().is_ok()
            && self.revision > 0
            && self.width > 0
            && self.height > 0
            && self.width <= 16384
            && self.height <= 16384
            && self.rotation <= 3
            && self
                .package_name
                .as_ref()
                .is_none_or(|p| p.len() <= 255 && !p.chars().any(char::is_control))
            && self.elements.len() <= MAX_MOBILE_ELEMENTS
            && self.elements.iter().all(|e| {
                e.element_ref.parse::<uuid::Uuid>().is_ok()
                    && e.role.len() <= 256
                    && e.name.as_ref().is_none_or(|n| n.len() <= 1024)
                    && (!e.protected || e.name.is_none())
                    && e.bounds.left >= 0
                    && e.bounds.top >= 0
                    && e.bounds.right >= e.bounds.left
                    && e.bounds.bottom >= e.bounds.top
                    && e.bounds.right <= self.width as i32
                    && e.bounds.bottom <= self.height as i32
            })
            && serde_json::to_vec(self).is_ok_and(|v| v.len() <= MAX_MOBILE_OBSERVATION_BYTES)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileState {
    Running,
    Completed,
    Failed,
    Cancelled,
    Unconfirmed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileReply {
    pub request_id: RequestId,
    pub state: MobileState,
    pub action_dispatched: bool,
    pub platform_acknowledged: bool,
    pub error: Option<String>,
    pub observation: Option<MobileObservation>,
    pub image: Option<MobileImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<MobileCapabilities>,
}
impl MobileReply {
    pub fn pending(request_id: RequestId) -> Self {
        Self {
            request_id,
            state: MobileState::Running,
            action_dispatched: false,
            platform_acknowledged: false,
            error: None,
            observation: None,
            image: None,
            capabilities: None,
        }
    }
    pub fn failed(request_id: RequestId, error: &str) -> Self {
        Self {
            state: MobileState::Failed,
            error: Some(error.into()),
            ..Self::pending(request_id)
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileImage {
    pub width: u32,
    pub height: u32,
    pub size: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobileCapabilities {
    pub schema_version: u16,
    pub actions: Vec<String>,
    pub max_elements: u16,
    pub max_text_bytes: u32,
    pub screenshot_requires_consent: bool,
}

impl MobileCapabilities {
    pub fn validate(&self) -> bool {
        self.schema_version == MOBILE_SCHEMA_VERSION
            && (1..=128).contains(&self.max_elements)
            && self.max_text_bytes <= 4096
            && !self.actions.is_empty()
            && self.actions.len() <= MOBILE_ACTIONS.len()
            && self
                .actions
                .iter()
                .all(|a| MOBILE_ACTIONS.contains(&a.as_str()))
            && self
                .actions
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                == self.actions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn additive_actions_negotiate_versions_and_bound_wait_query_and_launcher_input() {
        assert_eq!(MobileCommand::Observe {}.required_version(), 1);
        assert_eq!(MobileCommand::Capabilities {}.required_version(), 2);
        let wait = MobileCommand::WaitFor {
            text: "中文".into(),
            present: true,
            timeout_ms: 20000,
            interval_ms: 100,
        };
        assert!(wait.validate().is_ok());
        assert!(!wait.is_mutation());
        assert!(
            MobileCommand::WaitFor {
                text: "".into(),
                present: false,
                timeout_ms: 20000,
                interval_ms: 100
            }
            .validate()
            .is_err()
        );
        assert!(
            MobileCommand::Query {
                root_ref: None,
                text_contains: None,
                role: None,
                max_nodes: 129,
                max_depth: 32
            }
            .validate()
            .is_err()
        );
        for package in [
            "com.example/.Activity",
            "intent://launch",
            "com..example",
            "com.1invalid",
        ] {
            assert!(
                MobileCommand::LaunchApp {
                    package_name: package.into()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            MobileCommand::LaunchApp {
                package_name: "com.android.settings".into()
            }
            .validate()
            .is_ok()
        );
        let legacy = serde_json::to_value(MobileReply::pending(RequestId::new())).unwrap();
        assert!(legacy.get("capabilities").is_none());
        assert!(serde_json::from_value::<MobileReply>(legacy).is_ok());
    }
    #[test]
    fn mobile_contract_rejects_unbounded_and_ambiguous_input() {
        let reference = RequestId::new().to_string();
        assert!(
            MobileCommand::SetText {
                element_ref: reference.clone(),
                text: "中".repeat(1400)
            }
            .validate()
            .is_err()
        );
        assert!(
            MobileCommand::Click {
                element_ref: "native-pointer".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_value::<MobileCommand>(
                serde_json::json!({"action":"observe","package":"other"})
            )
            .is_err()
        );
        assert!(
            MobileCommand::Gesture {
                observation_ref: reference,
                from_x: 0,
                from_y: 0,
                to_x: 1,
                to_y: 1,
                duration_ms: 0
            }
            .validate()
            .is_err()
        );
    }
}
