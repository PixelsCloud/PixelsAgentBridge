//! Typed audit whitelist. Request bodies, passwords and credentials have no field here.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy)]
pub enum AuditAction {
    Bootstrap,
    AccountUpdated,
    BandwidthUpdated,
    PasswordChanged,
    PasswordReset,
    SessionsRevoked,
    DeviceRenamed,
    DeviceAssociated,
    DeviceDisassociated,
    RelayMaintenance,
    RelayActivate,
    RelayDisable,
    RelayReload,
    RelayCapacityUpdated,
    UsageExport,
    AuditExport,
    RegistrationUpdated,
    RelayStateChange,
}
impl AuditAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bootstrap => "admin.bootstrap",
            Self::AccountUpdated => "account.updated",
            Self::BandwidthUpdated => "account.relay_limit_updated",
            Self::PasswordChanged => "account.password_changed",
            Self::PasswordReset => "account.password_reset",
            Self::SessionsRevoked => "account.sessions_revoked",
            Self::DeviceRenamed => "device.rename",
            Self::DeviceAssociated => "device.associated",
            Self::DeviceDisassociated => "device.disassociated",
            Self::RelayMaintenance => "relay_maintenance",
            Self::RelayActivate => "relay_activate",
            Self::RelayDisable => "relay_disable",
            Self::RelayReload => "relay_config_reloaded",
            Self::RelayCapacityUpdated => "relay.capacity_updated",
            Self::UsageExport => "usage.exported",
            Self::AuditExport => "audit.exported",
            Self::RegistrationUpdated => "service.registration_updated",
            Self::RelayStateChange => "relay.state_change",
        }
    }
}

#[derive(Clone)]
pub enum AuditResource {
    Account(Uuid),
    Device(Uuid),
    Relay(String),
    Server,
    Audit,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditRelayNode {
    pub node_id: String,
    pub weight: u32,
    pub capacity_mbps: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditValues {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_admin: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mbps: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions_revoked: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<AuditRelayNode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub associated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_source: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions_revoked: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub population: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_outage: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters_applied: Option<bool>,
}

pub struct AuditRecord {
    pub actor: Option<Uuid>,
    pub action: AuditAction,
    pub resource: AuditResource,
    pub request_id: Uuid,
    pub operation_id: Option<Uuid>,
    pub before: Option<AuditValues>,
    pub after: Option<AuditValues>,
    pub details: AuditDetails,
    pub result: &'static str,
}
impl AuditRecord {
    pub fn new(actor: Option<Uuid>, action: AuditAction, resource: AuditResource) -> Self {
        Self {
            actor,
            action,
            resource,
            request_id: Uuid::new_v4(),
            operation_id: None,
            before: None,
            after: None,
            details: AuditDetails::default(),
            result: "succeeded",
        }
    }
}
