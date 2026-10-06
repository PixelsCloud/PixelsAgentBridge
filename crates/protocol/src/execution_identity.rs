use crate::ExecutionContextRef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    #[default]
    Service,
    User,
    DesktopUser,
}

/// Public request selection. Identity claims are deliberately not accepted here:
/// the executor resolves an opaque, caller-bound reference to native account facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionSelection {
    Service {},
    User { context_ref: ExecutionContextRef },
    DesktopUser { context_ref: ExecutionContextRef },
}
impl Default for ExecutionSelection {
    fn default() -> Self {
        Self::Service {}
    }
}
impl ExecutionSelection {
    pub fn mode(self) -> ExecutionMode {
        match self {
            Self::Service {} => ExecutionMode::Service,
            Self::User { .. } => ExecutionMode::User,
            Self::DesktopUser { .. } => ExecutionMode::DesktopUser,
        }
    }
    pub fn context_ref(self) -> Option<ExecutionContextRef> {
        match self {
            Self::Service {} => None,
            Self::User { context_ref } | Self::DesktopUser { context_ref } => Some(context_ref),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionEnvironmentSource {
    ServiceProcess,
    NativeAccount,
    InteractiveSession,
}

/// Observed identity only: no credentials, token handles, private keys or full env.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionIdentity {
    pub mode: ExecutionMode,
    /// Windows SID or `uid:<decimal>` on Unix; never an environment variable.
    pub account_id: String,
    pub account_name: String,
    pub home: String,
    pub primary_group: Option<u32>,
    pub session_id: Option<String>,
    /// Windows AuthenticationId / a platform session generation when available.
    pub logon_id: Option<String>,
    pub environment_source: ExecutionEnvironmentSource,
}
impl ExecutionIdentity {
    pub fn validate(&self) -> Result<(), &'static str> {
        for (value, limit) in [
            (&self.account_id, 256),
            (&self.account_name, 256),
            (&self.home, 32768),
        ] {
            if value.trim().is_empty() || value.len() > limit || value.contains('\0') {
                return Err("invalid execution account field");
            }
        }
        for value in [&self.session_id, &self.logon_id].into_iter().flatten() {
            if value.is_empty() || value.len() > 256 || value.contains('\0') {
                return Err("invalid execution session field");
            }
        }
        if self.mode == ExecutionMode::DesktopUser && self.session_id.is_none() {
            return Err("desktop user requires a verified session");
        }
        match (self.mode, self.environment_source) {
            (ExecutionMode::Service, ExecutionEnvironmentSource::ServiceProcess)
            | (ExecutionMode::User, ExecutionEnvironmentSource::NativeAccount)
            | (ExecutionMode::DesktopUser, ExecutionEnvironmentSource::InteractiveSession) => {
                Ok(())
            }
            _ => Err("execution mode and environment source differ"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_requires_opaque_context_not_claimed_account_or_password() {
        assert_eq!(
            ExecutionSelection::default(),
            ExecutionSelection::Service {}
        );
        for input in [
            r#"{"mode":"user","username":"alice"}"#,
            r#"{"mode":"service","password":"test"}"#,
            r#"{"mode":"user"}"#,
            r#"{"mode":"desktop_user","context_ref":"latest"}"#,
        ] {
            assert!(
                serde_json::from_str::<ExecutionSelection>(input).is_err(),
                "{input}"
            );
        }
        let selected = ExecutionSelection::User {
            context_ref: ExecutionContextRef::new(),
        };
        assert_eq!(
            serde_json::from_str::<ExecutionSelection>(&serde_json::to_string(&selected).unwrap())
                .unwrap(),
            selected
        );
    }
}
