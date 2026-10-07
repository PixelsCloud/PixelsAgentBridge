//! Application discovery is a bounded snapshot, not a paginated process list.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppQuery {
    List { request: AppListRequest },
    Execute { request: AppActionRequest },
}
impl AppQuery {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::List { request } => request.validate(),
            Self::Execute { request } => request.validate(),
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::List { .. } => "app_list",
            Self::Execute {
                request: AppActionRequest::Launch { .. },
            } => "app_launch",
            Self::Execute { .. } => "app_open_file",
        }
    }
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Execute { .. })
    }
    pub fn required_helper_version(&self) -> u16 {
        if matches!(
            self,
            Self::Execute {
                request: AppActionRequest::Launch {
                    new_instance: true,
                    ..
                }
            }
        ) {
            2
        } else {
            1
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppSnapshot {
    List { snapshot: AppListSnapshot },
    Action { result: AppActionResult },
}

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
        /// macOS NSWorkspace option; never silently ignored on other platforms.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        new_instance: bool,
    },
    OpenFile {
        path: String,
        application: Option<AppTarget>,
    },
}
impl AppActionRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Launch { application, .. } => application.validate(),
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

#[cfg(test)]
mod instance_policy_tests {
    use super::*;
    #[test]
    fn default_wire_shape_and_new_instance_capability_are_distinct() {
        let old =
            serde_json::json!({"operation":"launch","application":{"kind":"id","id":"fixture"}});
        let request: AppActionRequest = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(&request).unwrap(), old);
        assert_eq!(AppQuery::Execute { request }.required_helper_version(), 1);
        let new = AppQuery::Execute {
            request: AppActionRequest::Launch {
                application: AppTarget::Id {
                    id: "fixture".into(),
                },
                new_instance: true,
            },
        };
        assert_eq!(new.required_helper_version(), 2);
        assert_eq!(
            serde_json::to_value(&new).unwrap()["request"]["new_instance"],
            true
        );
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
    fn application_query_requires_explicit_desktop_selection_and_version_12() {
        use crate::{ExecutionContextRef, ExecutionSelection, SystemQuery};
        let app = AppQuery::Execute {
            request: AppActionRequest::OpenFile {
                path: "/tmp/example.txt".into(),
                application: None,
            },
        };
        for execution in [
            ExecutionSelection::Service {},
            ExecutionSelection::User {
                context_ref: ExecutionContextRef::new(),
            },
        ] {
            assert!(
                SystemQuery::Applications {
                    execution,
                    query: app.clone()
                }
                .validate()
                .is_err()
            );
        }
        let query = SystemQuery::Applications {
            execution: ExecutionSelection::DesktopUser {
                context_ref: ExecutionContextRef::new(),
            },
            query: app,
        };
        assert!(query.validate().is_ok());
        assert_eq!(query.required_version(), 12);
        assert!(query.is_mutation());
        assert_eq!(query.kind(), "app_open_file");
        assert_eq!(
            serde_json::from_value::<SystemQuery>(serde_json::to_value(&query).unwrap()).unwrap(),
            query
        );
    }
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
                    new_instance: false,
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
