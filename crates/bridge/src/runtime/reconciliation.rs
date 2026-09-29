use std::{path::Path, sync::Arc, time::Duration};

use pab_protocol::{RequestId, TransferSnapshot};
use sha2::{Digest, Sha256};
use tokio::{fs::File, io::AsyncReadExt, sync::watch};

use super::{RuntimeError, RuntimeInner, device::BridgeAvailability, operation::OperationRecord};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
const QUERY_BATCH: u32 = 8;

pub(super) async fn run_reconciliation(
    inner: Arc<RuntimeInner>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(RECONCILE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut cancellation_cursor: Option<(i64, String)> = None;
    let mut stale_cursor: Option<(i64, String)> = None;

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = inner.reconciliation_notify.notified() => {
                cancellation_cursor = None;
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
                continue;
            }
        }

        let cancellations = match inner
            .store
            .cancellation_requests(
                QUERY_BATCH,
                cancellation_cursor
                    .as_ref()
                    .map(|(started_at, id)| (*started_at, id.as_str())),
            )
            .await
        {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(%error, "failed to load cancellation requests");
                continue;
            }
        };
        cancellation_cursor = next_cursor(&cancellations);
        let stale = match inner
            .store
            .unconfirmed_transfers(
                QUERY_BATCH,
                stale_cursor
                    .as_ref()
                    .map(|(started_at, id)| (*started_at, id.as_str())),
            )
            .await
        {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(%error, "failed to load unconfirmed transfers");
                continue;
            }
        };
        stale_cursor = next_cursor(&stale);

        for record in cancellations.into_iter().chain(stale) {
            let Ok(request_id) = record.id.parse::<RequestId>() else {
                tracing::warn!(operation_id = %record.id, "invalid stored transfer request ID");
                continue;
            };
            let query = async {
                refresh_guest_grant(&inner, &record).await?;
                let device = inner.device(record.device_ref).await;
                let connection = device.connection().await?;
                connection
                    .get_transfer(request_id)
                    .await
                    .map_err(RuntimeError::from)
            };
            let remote = tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                    continue;
                }
                result = tokio::time::timeout(QUERY_TIMEOUT, query) => result,
            };
            let snapshot = match remote {
                Ok(Ok(snapshot)) => snapshot,
                Ok(Err(error)) => {
                    tracing::debug!(operation_id = %record.id, %error, "transfer lookup unavailable");
                    continue;
                }
                Err(_) => {
                    tracing::debug!(operation_id = %record.id, "transfer lookup timed out");
                    continue;
                }
            };
            if record.direction == "upload"
                && remote_target_matches(&record, &snapshot)
                && let Err(error) = inner
                    .store
                    .operation_progress(request_id, snapshot.offset, snapshot.size)
                    .await
            {
                tracing::warn!(%error, %request_id, "failed to save observed upload progress");
            }
            if remote_incomplete_upload_failed(&record, &snapshot) {
                match inner
                    .store
                    .finish_operation(request_id, "failed", snapshot.message.as_deref())
                    .await
                {
                    Ok(true) => tracing::info!(%request_id, "confirmed incomplete upload failure"),
                    Ok(false) => {}
                    Err(error) => {
                        tracing::warn!(%error, %request_id, "failed to save upload failure");
                    }
                }
                continue;
            }
            if !remote_completion_matches(&record, &snapshot) {
                continue;
            }
            if record.direction == "download" {
                let Some(digest) = snapshot.sha256.as_deref() else {
                    continue;
                };
                let verified = tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            return;
                        }
                        continue;
                    }
                    result = published_file_matches(Path::new(&record.destination), snapshot.size, digest) => result,
                };
                if !verified {
                    continue;
                }
            }
            if let Err(error) = inner
                .store
                .operation_progress(request_id, snapshot.offset, snapshot.size)
                .await
            {
                tracing::warn!(%error, %request_id, "failed to save reconciled upload progress");
                continue;
            }
            match inner
                .store
                .finish_operation(request_id, "completed", None)
                .await
            {
                Ok(true) => tracing::info!(%request_id, "reconciled completed transfer"),
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(%error, %request_id, "failed to finish reconciled transfer");
                }
            }
        }
    }
}

fn next_cursor(records: &[OperationRecord]) -> Option<(i64, String)> {
    if records.len() < QUERY_BATCH as usize {
        return None;
    }
    records
        .last()
        .map(|record| (record.started_at_unix_ms, record.id.clone()))
}

async fn refresh_guest_grant(
    inner: &Arc<RuntimeInner>,
    record: &OperationRecord,
) -> Result<(), RuntimeError> {
    if !inner.guest_identity {
        return Ok(());
    }
    let code = record.device_code.ok_or_else(|| {
        RuntimeError::BridgeUnavailable("transfer has no saved guest device code".to_owned())
    })?;
    if inner.guest_codes.lock().await.contains(&code) {
        return Ok(());
    }
    let BridgeAvailability::Connected(connector) = inner.availability.borrow().clone() else {
        return Err(RuntimeError::BridgeUnavailable(
            "Bridge is not connected for guest transfer recovery".to_owned(),
        ));
    };
    let resolved = connector.resolve_device_code(code).await?;
    if resolved != record.device_ref {
        return Err(RuntimeError::BridgeUnavailable(
            "saved device code no longer resolves to the transfer target".to_owned(),
        ));
    }
    inner.start_guest_lease(code).await;
    Ok(())
}

fn remote_completion_matches(record: &OperationRecord, snapshot: &TransferSnapshot) -> bool {
    remote_target_matches(record, snapshot)
        && snapshot.state == "completed"
        && snapshot.offset == snapshot.size
        && snapshot.finished_at_unix_ms.is_some()
}

fn remote_incomplete_upload_failed(record: &OperationRecord, snapshot: &TransferSnapshot) -> bool {
    record.direction == "upload"
        && remote_target_matches(record, snapshot)
        && snapshot.state == "failed"
        && snapshot.offset < snapshot.size
        && snapshot.finished_at_unix_ms.is_some()
}

fn remote_target_matches(record: &OperationRecord, snapshot: &TransferSnapshot) -> bool {
    match record.direction.as_str() {
        "upload" => snapshot.direction == "receive" && snapshot.path == record.destination,
        "download" => snapshot.direction == "send" && snapshot.path == record.source,
        _ => false,
    }
}

async fn published_file_matches(path: &Path, size: u64, digest: &str) -> bool {
    let Ok(metadata) = tokio::fs::symlink_metadata(path).await else {
        return false;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != size {
        return false;
    }
    let Ok(mut file) = File::open(path).await else {
        return false;
    };
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let Ok(read) = file.read(&mut buffer).await else {
            return false;
        };
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    format!("{:x}", hash.finalize()) == digest
}

#[cfg(test)]
mod tests {
    use pab_protocol::{DeploymentId, DeviceId, DeviceRef, OperatorRef, TenantId, UserId};

    use super::*;

    #[tokio::test]
    async fn download_recovery_requires_the_published_file_checksum() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("download.bin");
        tokio::fs::write(&path, b"verified bytes").await.unwrap();
        let checksum = format!("{:x}", Sha256::digest(b"verified bytes"));
        assert!(published_file_matches(&path, 14, &checksum).await);
        assert!(!published_file_matches(&path, 13, &checksum).await);
        assert!(!published_file_matches(&path, 14, &"0".repeat(64)).await);
    }

    #[test]
    fn remote_completion_must_match_the_transfer_direction_and_path() {
        let request_id = RequestId::from_u128(1);
        let actor = OperatorRef::account(
            UserId::from_u128(2),
            pab_protocol::EndpointKey::new([2; 32]),
        );
        let record = OperationRecord {
            id: request_id.to_string(),
            device_ref: DeviceRef {
                deployment_id: DeploymentId::from_u128(3),
                tenant_id: TenantId::from_u128(4),
                device_id: DeviceId::from_u128(5),
            },
            device_code: None,
            initiated_by: "guest".to_owned(),
            kind: "file_transfer".to_owned(),
            direction: "download".to_owned(),
            source: "/tmp/source.bin".to_owned(),
            destination: "local.bin".to_owned(),
            overwrite: false,
            state: "running".to_owned(),
            offset: 0,
            size: 0,
            started_at_unix_ms: 1,
            finished_at_unix_ms: None,
            message: None,
            execution_observation: Some("unconfirmed".to_owned()),
        };
        let mut snapshot = TransferSnapshot {
            request_id,
            initiated_by: actor,
            direction: "send".to_owned(),
            path: "/tmp/source.bin".to_owned(),
            state: "completed".to_owned(),
            offset: 14,
            size: 14,
            sha256: Some("a".repeat(64)),
            finished_at_unix_ms: Some(2),
            message: None,
        };
        assert!(remote_completion_matches(&record, &snapshot));
        snapshot.path = "/tmp/other.bin".to_owned();
        assert!(!remote_completion_matches(&record, &snapshot));
        snapshot.path = record.source.clone();
        snapshot.direction = "receive".to_owned();
        assert!(!remote_completion_matches(&record, &snapshot));

        let mut upload = record.clone();
        upload.direction = "upload".to_owned();
        upload.destination = snapshot.path.clone();
        snapshot.state = "failed".to_owned();
        snapshot.offset = 1;
        assert!(remote_incomplete_upload_failed(&upload, &snapshot));
        snapshot.offset = snapshot.size;
        assert!(!remote_incomplete_upload_failed(&upload, &snapshot));
    }
}
