//! Application discovery is a bounded snapshot, not a paginated process list.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppTarget {
    Id { id: String },
    Path { path: String },
}
impl AppTarget {
    pub fn validate(&self) -> Result<(), &'static str> {
        let value = match self {
            Self::Id { id } => id,
            Self::Path { path } => path,
        };
        if value.trim().is_empty() || value.len() > 4096 || value.contains('\0') {
            return Err("application identifier/path must be 1..4096 UTF-8 bytes without NUL");
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppActionRequest {
    Launch {
        application: AppTarget,
    },
    OpenFile {
        path: String,
        application: Option<AppTarget>,
    },
}
impl AppActionRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Launch { application } => application.validate(),
            Self::OpenFile { path, application } => {
                if path.trim().is_empty() || path.len() > 4096 || path.contains('\0') {
                    return Err("local file path must be 1..4096 UTF-8 bytes without NUL");
                }
                if let Some(app) = application {
                    app.validate()?;
                }
                Ok(())
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppActionResult {
    pub request_accepted: bool,
    pub instance: Option<AppInstance>,
    /// None means no reliable evidence that the OS reused or created an instance.
    pub reused_instance: Option<bool>,
    pub execution_identity: crate::ExecutionIdentity,
    /// Activation completion does not prove the application window is ready.
    pub window_ready: Option<bool>,
    pub notes: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppActionError {
    pub action_started: bool,
    pub message: String,
}
impl AppActionError {
    pub fn new(action_started: bool, message: impl ToString) -> Self {
        let mut message = message.to_string();
        if message.len() > 4096 {
            let mut end = 4096;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
        }
        Self {
            action_started,
            message,
        }
    }
}
impl std::fmt::Display for AppActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for AppActionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppListScope {
    Installed,
    Running,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppListRequest {
    pub scope: AppListScope,
    #[serde(default)]
    pub search: String,
    pub limit: u16,
}
impl AppListRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.search.len() > 256 || self.search.contains('\0') {
            return Err("app search must be at most 256 UTF-8 bytes without NUL");
        }
        if !(1..=200).contains(&self.limit) {
            return Err("app list limit must be between 1 and 200; narrow search when truncated");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInstance {
    pub process_id: u32,
    pub process_identity: String,
    pub account_id: Option<String>,
    pub session_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    /// OS identifier: Shell parsing name on Windows, bundle ID on macOS.
    pub app_id: Option<String>,
    pub path: Option<String>,
    pub source: String,
    pub instance: Option<AppInstance>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppListSnapshot {
    pub apps: Vec<AppInfo>,
    pub truncated: bool,
    pub sources: Vec<String>,
    pub warnings: Vec<String>,
    pub execution_identity: crate::ExecutionIdentity,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn application_actions_reject_extra_arguments_and_malformed_targets() {
        for value in [
            serde_json::json!({"operation":"launch","application":{"kind":"path","path":"/Applications/Test.app"},"args":["--unexpected"]}),
            serde_json::json!({"operation":"launch","application":{"kind":"id","id":"test","username":"other"}}),
            serde_json::json!({"operation":"open_file","path":"/tmp/test","application":null,"url":"https://example.com"}),
        ] {
            assert!(serde_json::from_value::<AppActionRequest>(value).is_err());
        }
        for id in [String::new(), "x\0y".into(), "中".repeat(1366)] {
            assert!(
                AppActionRequest::Launch {
                    application: AppTarget::Id { id }
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            AppActionRequest::OpenFile {
                path: "/tmp/中文 文件.txt".into(),
                application: None
            }
            .validate()
            .is_ok()
        );
    }
    #[test]
    fn app_discovery_is_bounded_and_rejects_unknown_paging_fields() {
        let mut q = AppListRequest {
            scope: AppListScope::Running,
            search: "中".repeat(85),
            limit: 200,
        };
        assert!(q.validate().is_ok());
        q.search.push('中');
        assert!(q.validate().is_err());
        q.search.clear();
        q.limit = 0;
        assert!(q.validate().is_err());
        q.limit = 201;
        assert!(q.validate().is_err());
        assert!(
            serde_json::from_value::<AppListRequest>(
                serde_json::json!({"scope":"running", "limit":20,"page":2})
            )
            .is_err()
        );
    }
}
