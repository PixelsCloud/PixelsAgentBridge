//! Application discovery is a bounded snapshot, not a paginated process list.
use serde::{Deserialize, Serialize};

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
