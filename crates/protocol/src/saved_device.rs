use crate::{DeviceId, DeviceRef};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedDevice {
    pub device_ref: DeviceRef,
    pub code: String,
    pub name: String,
    pub system: Option<String>,
    pub alias: String,
    pub revision: i64,
    pub deleted: bool,
    /// None means the current access verification is no longer valid.
    pub online: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedDeviceChanges {
    pub items: Vec<SavedDevice>,
    pub cursor: i64,
    pub has_more: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedDeviceMutation {
    pub id: Uuid,
    pub device_id: DeviceId,
    pub expected_revision: i64,
    pub alias: String,
    pub deleted: bool,
}

/// Import of a local snapshot only. It never proves live access to the target.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedDeviceImport {
    pub device_ref: DeviceRef,
    pub code: String,
    pub name: String,
    pub system: Option<String>,
    pub alias: String,
}
