use serde::{Deserialize, Serialize};

/// Notification contents are transient samples. References are caller/connection bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotificationRequest {
    Status {},
    List {
        package_id: String,
        limit: u16,
    },
    Wait {
        package_id: String,
        after_revision: u64,
        timeout_ms: u32,
        limit: u16,
    },
    Open {
        notification_ref: String,
    },
    Dismiss {
        notification_ref: String,
    },
}
impl NotificationRequest {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Status {} => "notification_status",
            Self::List { .. } => "notification_list",
            Self::Wait { .. } => "notification_wait",
            Self::Open { .. } => "notification_open",
            Self::Dismiss { .. } => "notification_dismiss",
        }
    }
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Open { .. } | Self::Dismiss { .. })
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::List { package_id, limit }
            | Self::Wait {
                package_id, limit, ..
            } => {
                if package_id.len() > 255
                    || package_id.split('.').count() < 2
                    || package_id.split('.').any(|p| {
                        p.is_empty() || !p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    })
                    || !(1..=32).contains(limit)
                {
                    return Err("notification package_id and limit 1..32 required");
                }
            }
            Self::Open { notification_ref } | Self::Dismiss { notification_ref } => {
                notification_ref
                    .parse::<crate::RequestId>()
                    .map_err(|_| "notification_ref must be an opaque UUID")?;
            }
            _ => {}
        }
        if let Self::Wait {
            timeout_ms,
            after_revision,
            ..
        } = self
            && (!(100..=30000).contains(timeout_ms) || *after_revision > i64::MAX as u64)
        {
            return Err("notification wait timeout 100..30000 and signed revision required");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationEntry {
    pub notification_ref: String,
    pub package_id: String,
    pub title: Option<String>,
    pub text: Option<String>,
    pub posted_at_unix_ms: i64,
    pub ongoing: bool,
    pub clearable: bool,
    pub can_open: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSnapshot {
    pub access_granted: bool,
    pub listener_connected: bool,
    pub revision: u64,
    pub entries: Vec<NotificationEntry>,
    pub outcome: String,
    pub action_dispatched: bool,
    pub truncated: bool,
}
impl NotificationSnapshot {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.entries.len() > 32
            || self.revision > i64::MAX as u64
            || ![
                "sampled",
                "changed",
                "timed_out",
                "dispatched",
                "removed",
                "unconfirmed",
            ]
            .contains(&self.outcome.as_str())
        {
            return Err("invalid notification snapshot");
        }
        for entry in &self.entries {
            entry
                .notification_ref
                .parse::<crate::RequestId>()
                .map_err(|_| "invalid notification reference")?;
            if entry.package_id.len() > 255
                || entry.title.as_ref().is_some_and(|v| v.len() > 1024)
                || entry.text.as_ref().is_some_and(|v| v.len() > 4096)
            {
                return Err("notification field budget exceeded");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notifications_require_explicit_package_and_bounded_waits_and_refs() {
        assert!(
            NotificationRequest::List {
                package_id: "com.pixels.fixture".into(),
                limit: 32
            }
            .validate()
            .is_ok()
        );
        assert!(
            NotificationRequest::List {
                package_id: "*".into(),
                limit: 1
            }
            .validate()
            .is_err()
        );
        assert!(
            NotificationRequest::Wait {
                package_id: "com.pixels.fixture".into(),
                limit: 1,
                timeout_ms: 30001,
                after_revision: 0
            }
            .validate()
            .is_err()
        );
        assert!(
            NotificationRequest::Dismiss {
                notification_ref: "system-key".into()
            }
            .validate()
            .is_err()
        );
        let query = crate::SystemQuery::Notifications {
            query: NotificationRequest::Status {},
        };
        assert_eq!(query.required_version(), 17);
        assert!(!query.is_mutation());
        assert_eq!(
            crate::SystemQuery::Info {
                include_gpu: false,
                sample_cpu: false
            }
            .required_version(),
            1
        );
    }
}
