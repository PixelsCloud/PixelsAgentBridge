use crate::UserId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageCounters {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub p2p_upload_bytes: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub p2p_download_bytes: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub p2p_reported: bool,
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
    pub fn values(&self) -> [u64; 12] {
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
            self.p2p_upload_bytes,
            self.p2p_download_bytes,
        ]
    }
    pub fn add(&mut self, other: &Self) {
        macro_rules! add {($($field:ident),*)=>{$(match self.$field.checked_add(other.$field){Some(n)=>self.$field=n,None=>{self.$field=u64::MAX;self.incomplete=true;}})*};}
        add!(
            p2p_upload_bytes,
            p2p_download_bytes,
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
        self.p2p_reported |= other.p2p_reported;
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}
fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_batches_keep_their_serialized_receipt_digest_and_new_fields_accumulate() {
        let old = r#"{"relay_upload_bytes":5,"relay_download_bytes":6,"connections":0,"connection_ms":0,"uploaded_files":0,"downloaded_files":0,"uploaded_bytes":0,"downloaded_bytes":0,"failed_transfers":0,"cancelled_transfers":0,"incomplete":false}"#;
        let mut counters: UsageCounters = serde_json::from_str(old).unwrap();
        assert_eq!(serde_json::to_string(&counters).unwrap(), old);
        counters.add(&UsageCounters {
            p2p_upload_bytes: 100,
            p2p_download_bytes: 200,
            p2p_reported: true,
            ..Default::default()
        });
        assert_eq!(
            (
                counters.relay_upload_bytes,
                counters.p2p_upload_bytes,
                counters.p2p_download_bytes
            ),
            (5, 100, 200)
        );
        assert!(counters.p2p_reported);
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
