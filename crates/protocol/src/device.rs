use serde::{Deserialize, Serialize};

use crate::{DeviceCode, DeviceRef, ExecutionContext, TenantId};

pub const DEVICE_SESSION_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceHello {
    pub schema_version: u16,
    pub device_ref: DeviceRef,
    pub execution_context: ExecutionContext,
    pub agent_version: String,
    pub observed_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceHelloResult {
    pub device_ref: DeviceRef,
    pub environment_revision: String,
    pub accepted_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceDirectoryEntry {
    pub device_ref: DeviceRef,
    pub code: DeviceCode,
    pub name: String,
    pub owner_tenant_id: TenantId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePresence {
    pub code: DeviceCode,
    pub name: String,
    pub online: bool,
}
