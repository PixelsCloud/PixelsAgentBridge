//! Package changes share application routing and request deduplication. A completed
//! dispatch is not an installed package: poll the caller-scoped change reference.
use crate::{ExecutionIdentity, RequestId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppPackageRequest {
    Inspect {
        path: String,
    },
    Install {
        path: String,
        package_id: String,
        sha256: String,
    },
    Uninstall {
        package_id: String,
        version_code: u64,
    },
    Status {
        change_ref: RequestId,
    },
    Resume {
        change_ref: RequestId,
    },
}

impl AppPackageRequest {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Inspect { .. } => "app_package_inspect",
            Self::Install { .. } => "app_install",
            Self::Uninstall { .. } => "app_uninstall",
            Self::Status { .. } => "app_change_status",
            Self::Resume { .. } => "app_change_resume",
        }
    }
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::Install { .. } | Self::Uninstall { .. } | Self::Resume { .. }
        )
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Self::Inspect { path } | Self::Install { path, .. } = self {
            if !path.starts_with('/') || path.len() > 4096 || path.contains('\0') {
                return Err("package path must be an absolute authorized path, at most 4096 bytes");
            }
        }
        if let Self::Install { package_id, .. } | Self::Uninstall { package_id, .. } = self {
            if !valid_package_id(package_id) {
                return Err("invalid package identifier");
            }
        }
        if let Self::Install { sha256, .. } = self {
            if !valid_package_sha256(sha256) {
                return Err("expected lowercase APK SHA-256 required");
            }
        }
        if let Self::Uninstall { version_code, .. } = self {
            if *version_code == 0 || *version_code > i64::MAX as u64 {
                return Err("installed version_code must be 1..i64::MAX");
            }
        }
        Ok(())
    }
}

pub fn valid_package_id(value: &str) -> bool {
    value.len() <= 255
        && value.contains('.')
        && value.split('.').all(|part| {
            part.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        })
}
pub fn valid_package_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppPackageInfo {
    pub package_id: String,
    pub version_name: Option<String>,
    pub version_code: u64,
    pub sha256: String,
    pub signer_sha256: Vec<String>,
    pub installed_version_code: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppPackageState {
    Inspected,
    Submitted,
    WaitingUserAction,
    Succeeded,
    Cancelled,
    Failed,
    Unconfirmed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppPackageResult {
    pub change_ref: Option<RequestId>,
    pub state: AppPackageState,
    pub package: Option<AppPackageInfo>,
    pub package_id: String,
    pub error: Option<String>,
    pub can_request_installs: bool,
    pub execution_identity: ExecutionIdentity,
    pub notes: Vec<String>,
}
impl AppPackageResult {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_package_id(&self.package_id)
            || self.notes.len() > 8
            || self.notes.iter().any(|s| s.len() > 1024)
            || self.error.as_ref().is_some_and(|s| s.len() > 256)
            || (self.state != AppPackageState::Inspected && self.change_ref.is_none())
            || (self.state == AppPackageState::Inspected
                && (self.change_ref.is_some() || self.package.is_none()))
        {
            return Err("invalid package result");
        }
        if let Some(info) = &self.package {
            if info.package_id != self.package_id
                || !valid_package_sha256(&info.sha256)
                || info.signer_sha256.is_empty()
                || info.signer_sha256.len() > 16
                || info.signer_sha256.iter().any(|s| !valid_package_sha256(s))
                || info.version_name.as_ref().is_some_and(|s| s.len() > 512)
            {
                return Err("invalid package metadata");
            }
        }
        self.execution_identity.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packages_require_pins_and_have_separate_observation_semantics() {
        let install = AppPackageRequest::Install {
            path: "/workspace/test.apk".into(),
            package_id: "com.pixels.fixture".into(),
            sha256: "a".repeat(64),
        };
        assert!(install.validate().is_ok());
        assert!(install.is_mutation());
        for value in ["", "fixture", "com..fixture", "com.0test", "com.test;id"] {
            assert!(!valid_package_id(value));
        }
        assert!(!valid_package_sha256(&"A".repeat(64)));
        let query = crate::AppQuery::Package {
            request: AppPackageRequest::Status {
                change_ref: RequestId::new(),
            },
        };
        assert!(!query.is_mutation());
        assert_eq!(query.kind(), "app_change_status");
        let selection = crate::ExecutionSelection::DesktopUser {
            context_ref: crate::ExecutionContextRef::new(),
        };
        assert_eq!(
            crate::SystemQuery::Applications {
                execution: selection,
                query
            }
            .required_version(),
            16
        );
        assert!(serde_json::from_value::<AppPackageRequest>(serde_json::json!({"operation":"install","path":"/workspace/test.apk","package_id":"com.pixels.fixture","sha256":"a".repeat(64),"silent":true})).is_err());
    }
}
