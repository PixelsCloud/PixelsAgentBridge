use crate::UserId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageCounters {
    pub relay_upload_bytes: u64,
    pub relay_download_bytes: u64,
    pub connections: u64,
    pub connection_ms: u64,
    pub uploaded_files: u64,
    pub downloaded_files: u64,
    pub uploaded_bytes: u64,
    pub downloaded_bytes: u64,
    pub failed_transfers: u64,
    pub cancelled_transfers: u64,
    pub incomplete: bool,
}

impl UsageCounters {
    pub fn values(&self) -> [u64; 10] {
        [
            self.relay_upload_bytes,
            self.relay_download_bytes,
            self.connections,
            self.connection_ms,
            self.uploaded_files,
            self.downloaded_files,
            self.uploaded_bytes,
            self.downloaded_bytes,
            self.failed_transfers,
            self.cancelled_transfers,
        ]
    }
    pub fn add(&mut self, other: &Self) {
        macro_rules! add {($($field:ident),*)=>{$(match self.$field.checked_add(other.$field){Some(n)=>self.$field=n,None=>{self.$field=u64::MAX;self.incomplete=true;}})*};}
        add!(
            relay_upload_bytes,
            relay_download_bytes,
            connections,
            connection_ms,
            uploaded_files,
            downloaded_files,
            uploaded_bytes,
            downloaded_bytes,
            failed_transfers,
            cancelled_transfers
        );
        self.incomplete |= other.incomplete;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageBatch {
    pub id: Uuid,
    pub user_id: Option<UserId>,
    pub hour_unix_ms: i64,
    pub counters: UsageCounters,
}

impl UsageBatch {
    pub fn valid_at(&self, now: i64) -> bool {
        self.hour_unix_ms >= 0
            && self.hour_unix_ms % 3_600_000 == 0
            && self.hour_unix_ms <= now
            && self.hour_unix_ms >= now - 30 * 86_400_000
            && self.counters.values().iter().all(|v| *v <= i64::MAX as u64)
    }
}
