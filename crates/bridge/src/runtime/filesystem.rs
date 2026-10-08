use pab_protocol::{DeviceCode, DeviceRef, FileSystemReply, FileSystemRequest, RequestId};
use sha2::{Digest, Sha256};
use sqlx::Row;

use super::{BridgeRuntime, RuntimeError, RuntimeStore, RuntimeStoreError, operation::now_unix_ms};
use crate::FileSystemResult;

impl RuntimeStore {
    pub(super) async fn owns_filesystem(
        &self,
        id: RequestId,
        owner: &str,
    ) -> Result<bool, RuntimeStoreError> {
        Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runtime_operations o JOIN runtime_filesystem_results f ON f.id=o.id WHERE o.id = ? AND o.owner_session_id = ?").bind(id.to_string()).bind(owner).fetch_one(&self.pool).await? == 1)
    }

    pub(super) async fn filesystem_record(
        &self,
        id: RequestId,
        code: Option<DeviceCode>,
        actor: &str,
        owner: &str,
    ) -> Result<(DeviceRef, FileSystemReply), RuntimeStoreError> {
        let row = sqlx::query("SELECT o.device_ref_json, o.device_code, o.owner_session_id, s.stopped_at_unix_ms, s.heartbeat_at_unix_ms, f.reply_json FROM runtime_operations o JOIN runtime_filesystem_results f ON f.id = o.id LEFT JOIN runtime_sessions s ON s.id = o.owner_session_id WHERE o.id = ? AND o.initiated_by = ?")
            .bind(id.to_string()).bind(actor).fetch_optional(&self.pool).await?.ok_or(RuntimeStoreError::NotFound)?;
        if code.is_some_and(|code| {
            row.try_get::<Option<String>, _>("device_code")
                .ok()
                .flatten()
                .as_deref()
                != Some(code.to_string().as_str())
        }) {
            return Err(RuntimeStoreError::NotFound);
        }
        if row
            .try_get::<Option<String>, _>("owner_session_id")?
            .as_deref()
            != Some(owner)
            && row
                .try_get::<Option<i64>, _>("stopped_at_unix_ms")?
                .is_none()
            && row
                .try_get::<Option<i64>, _>("heartbeat_at_unix_ms")?
                .is_some_and(|time| time >= now_unix_ms() - 15_000)
        {
            return Err(RuntimeStoreError::NotFound);
        }
        Ok((
            serde_json::from_str(row.try_get("device_ref_json")?)?,
            serde_json::from_str(row.try_get("reply_json")?)?,
        ))
    }

    async fn accept_filesystem(
        &self,
        request: &FileSystemRequest,
        device: DeviceRef,
        code: Option<DeviceCode>,
        actor: &str,
        owner: &str,
    ) -> Result<bool, RuntimeStoreError> {
        let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(request)?));
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing = sqlx::query("SELECT o.device_ref_json, o.initiated_by, f.fingerprint FROM runtime_operations o LEFT JOIN runtime_filesystem_results f ON f.id = o.id WHERE o.id = ?").bind(request.request_id.to_string()).fetch_optional(&mut *tx).await?;
        if let Some(row) = existing {
            if serde_json::from_str::<DeviceRef>(row.try_get("device_ref_json")?)? != device
                || row.try_get::<String, _>("initiated_by")? != actor
                || row.try_get::<Option<String>, _>("fingerprint")?.as_deref() != Some(&fingerprint)
            {
                return Err(RuntimeStoreError::RequestConflict);
            }
            tx.commit().await?;
            self.filesystem_record(request.request_id, code, actor, owner)
                .await?;
            return Ok(false);
        }
        let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runtime_operations o JOIN runtime_filesystem_results f ON f.id=o.id WHERE o.owner_session_id = ? AND o.finished_at_unix_ms IS NULL").bind(owner).fetch_one(&mut *tx).await?;
        if active >= 32 {
            return Err(RuntimeStoreError::FilesystemBusy);
        }
        let mut pending = FileSystemReply::pending(request);
        let existing_command: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM runtime_tasks WHERE request_id = ?")
                .bind(request.request_id.to_string())
                .fetch_one(&mut *tx)
                .await?;
        if existing_command != 0 {
            return Err(RuntimeStoreError::RequestConflict);
        }
        pending.state = "unconfirmed".to_owned();
        sqlx::query("INSERT INTO runtime_operations (id, device_ref_json, device_code, initiated_by, kind, direction, source, destination, overwrite, state, started_at_unix_ms, owner_session_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'running', ?, ?)")
            .bind(request.request_id.to_string()).bind(serde_json::to_string(&device)?).bind(code.map(|code| code.to_string())).bind(actor).bind(request.operation.kind()).bind(if request.operation.mutates() { "write" } else { "read" }).bind(&request.path).bind(request.operation.destination().unwrap_or("")).bind(matches!(request.operation, pab_protocol::FileSystemAction::Write { overwrite: true, .. } | pab_protocol::FileSystemAction::Patch { .. } | pab_protocol::FileSystemAction::Copy { overwrite: true, .. } | pab_protocol::FileSystemAction::Move { overwrite: true, .. } | pab_protocol::FileSystemAction::ArchiveCreate { overwrite: true, .. } | pab_protocol::FileSystemAction::ArchiveExtract { overwrite: true, .. })).bind(now_unix_ms()).bind(owner).execute(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO runtime_filesystem_results (id, fingerprint, reply_json) VALUES (?, ?, ?)",
        )
        .bind(request.request_id.to_string())
        .bind(fingerprint)
        .bind(serde_json::to_string(&pending)?)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn save_filesystem_reply(
        &self,
        reply: &FileSystemReply,
    ) -> Result<(), RuntimeStoreError> {
        let mut summary = reply.clone();
        summary.data_size = 0;
        summary.data_sha256 = None;
        let terminal = matches!(
            reply.state.as_str(),
            "completed" | "failed" | "interrupted" | "cancelled"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let prior = sqlx::query("SELECT o.state, f.reply_json FROM runtime_operations o JOIN runtime_filesystem_results f ON f.id = o.id WHERE o.id = ?")
            .bind(reply.request_id.to_string()).fetch_optional(&mut *tx).await?;
        if let Some(row) = prior {
            if row.try_get::<String, _>("state")? == "cancel_requested" && !terminal {
                summary.state = "cancel_requested".to_owned();
            }
            let stored: FileSystemReply = serde_json::from_str(row.try_get("reply_json")?)?;
            if stored.execution_context.is_some()
                && summary.execution_context != stored.execution_context
            {
                return Err(RuntimeStoreError::SnapshotIdentityMismatch);
            }
            match (&mut summary.progress, stored.progress) {
                (Some(current), Some(old)) => {
                    current.completed_bytes = current.completed_bytes.max(old.completed_bytes);
                    current.total_bytes = current.total_bytes.max(old.total_bytes);
                    current.updated_at_unix_ms =
                        current.updated_at_unix_ms.max(old.updated_at_unix_ms);
                }
                (None, Some(old)) => summary.progress = Some(old),
                _ => {}
            }
        }
        let count = reply
            .directory
            .as_ref()
            .map(|page| page.entries.len() as i64);
        let changed = sqlx::query("UPDATE runtime_operations SET state = ?, finished_at_unix_ms = ?, message = ?, size = ?, offset = ? WHERE id = ? AND state IN ('running', 'cancel_requested')")
            .bind(if terminal { reply.state.as_str() } else if summary.state == "cancel_requested" { "cancel_requested" } else { "running" }).bind(terminal.then(now_unix_ms)).bind(reply.error.as_ref().map(|error| format!("{} ({}): {}", error.code, error.phase, error.message))).bind(summary.progress.as_ref().map_or_else(|| count.unwrap_or_else(|| reply.metadata.as_ref().map_or(0, |meta| meta.size.min(i64::MAX as u64) as i64)), |progress| progress.total_bytes.min(i64::MAX as u64) as i64)).bind(summary.progress.as_ref().map_or(count.unwrap_or(reply.data_size as i64), |progress| progress.completed_bytes.min(i64::MAX as u64) as i64)).bind(reply.request_id.to_string()).execute(&mut *tx).await?;
        if changed.rows_affected() == 1 {
            sqlx::query("UPDATE runtime_filesystem_results SET reply_json = ? WHERE id = ?")
                .bind(serde_json::to_string(&summary)?)
                .bind(reply.request_id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

impl BridgeRuntime {
    pub async fn filesystem(
        &self,
        device: DeviceRef,
        request: &FileSystemRequest,
        payload: &[u8],
    ) -> Result<FileSystemResult, RuntimeError> {
        request
            .validate()
            .map_err(|error| crate::BridgeError::UnexpectedTaskResponse(error.to_owned()))?;
        if payload.len() != request.payload_size as usize
            || request.operation.has_payload()
                && request.payload_sha256.as_deref()
                    != Some(format!("{:x}", Sha256::digest(payload)).as_str())
        {
            return Err(crate::BridgeError::UnexpectedTaskResponse(
                "invalid local text payload".to_owned(),
            )
            .into());
        }
        self.inner.wait_account_ready().await?;
        self.inner.wait_account_ready().await?;
        let code = self.inner.device_codes.lock().await.get(&device).copied();
        if !self
            .inner
            .store
            .accept_filesystem(
                request,
                device,
                code,
                &self.inner.initiated_by,
                &self.inner.session_id,
            )
            .await?
        {
            if matches!(
                request.operation,
                pab_protocol::FileSystemAction::Read { .. }
            ) {
                return Err(RuntimeStoreError::RequestConflict.into());
            }
            let reply = self.get_filesystem(device, request.request_id).await?;
            return Ok(FileSystemResult {
                reply,
                data: Vec::new(),
            });
        }
        let connection = match self.inner.device(device).await.connection().await {
            Ok(connection) => connection,
            Err(error) => {
                let mut reply = FileSystemReply::pending(request);
                reply.state = "failed".to_owned();
                reply.error = Some(pab_protocol::FileSystemError {
                    code: "connection_unavailable".to_owned(),
                    phase: "connect".to_owned(),
                    message: error.to_string(),
                });
                self.inner.store.save_filesystem_reply(&reply).await?;
                return Err(error);
            }
        };
        match connection.filesystem(request, payload).await {
            Ok(result) => {
                self.inner
                    .store
                    .save_filesystem_reply(&result.reply)
                    .await?;
                Ok(result)
            }
            Err(error) => {
                // The remote may have published a mutation before the response
                // was lost. Preserve its original ID and never replay it here.
                let mut reply = FileSystemReply::pending(request);
                let rejected_context = matches!(
                    &error,
                    crate::BridgeError::RemoteTask {
                        code: pab_protocol::DeviceTaskErrorCode::EnvironmentChanged,
                        ..
                    }
                );
                reply.state = if (request.operation.mutates()
                    || matches!(request.operation, pab_protocol::FileSystemAction::Hash))
                    && !matches!(error, crate::BridgeError::UnsupportedFileSystem)
                    && !rejected_context
                {
                    "unconfirmed"
                } else {
                    "failed"
                }
                .to_owned();
                reply.error = Some(pab_protocol::FileSystemError {
                    code: if rejected_context {
                        "execution_context_unavailable"
                    } else if matches!(error, crate::BridgeError::UnsupportedFileSystem) {
                        "unsupported"
                    } else {
                        "response_unavailable"
                    }
                    .to_owned(),
                    phase: if rejected_context {
                        "accept"
                    } else {
                        "receive"
                    }
                    .to_owned(),
                    message: error.to_string(),
                });
                self.inner.store.save_filesystem_reply(&reply).await?;
                Ok(FileSystemResult {
                    reply,
                    data: Vec::new(),
                })
            }
        }
    }

    pub async fn cancel_filesystem(
        &self,
        device: DeviceRef,
        id: RequestId,
    ) -> Result<FileSystemReply, RuntimeError> {
        let (stored, cached) = self
            .inner
            .store
            .filesystem_record(id, None, &self.inner.initiated_by, &self.inner.session_id)
            .await?;
        if stored != device
            || !pab_protocol::cancellable_filesystem_kind(&cached.kind)
            || !self
                .inner
                .store
                .owns_filesystem(id, &self.inner.session_id)
                .await?
        {
            return Err(RuntimeStoreError::NotFound.into());
        }
        if matches!(
            cached.state.as_str(),
            "completed" | "failed" | "interrupted" | "cancelled"
        ) {
            return Ok(cached);
        }
        let connection = self.inner.device(device).await.connection().await?;
        let reply = connection.cancel_filesystem(id).await?;
        if reply.path != cached.path
            || reply.kind != cached.kind
            || reply.destination != cached.destination
        {
            return Err(RuntimeStoreError::SnapshotIdentityMismatch.into());
        }
        self.inner.store.save_filesystem_reply(&reply).await?;
        Ok(self
            .inner
            .store
            .filesystem_record(id, None, &self.inner.initiated_by, &self.inner.session_id)
            .await?
            .1)
    }

    pub async fn get_filesystem(
        &self,
        device: DeviceRef,
        id: RequestId,
    ) -> Result<FileSystemReply, RuntimeError> {
        let (stored_device, cached) = self
            .inner
            .store
            .filesystem_record(id, None, &self.inner.initiated_by, &self.inner.session_id)
            .await?;
        if device != stored_device {
            return Err(RuntimeStoreError::NotFound.into());
        }
        if matches!(
            cached.state.as_str(),
            "completed" | "failed" | "interrupted" | "cancelled"
        ) {
            return Ok(cached);
        }
        let query = async {
            self.inner
                .device(device)
                .await
                .connection()
                .await?
                .get_filesystem(id)
                .await
                .map_err(RuntimeError::from)
        };
        let Ok(Ok(reply)) = tokio::time::timeout(std::time::Duration::from_secs(5), query).await
        else {
            return Ok(cached);
        };
        if reply.path != cached.path
            || reply.kind != cached.kind
            || reply.destination != cached.destination
        {
            return Err(RuntimeStoreError::SnapshotIdentityMismatch.into());
        }
        self.inner.store.save_filesystem_reply(&reply).await?;
        Ok(self
            .inner
            .store
            .filesystem_record(id, None, &self.inner.initiated_by, &self.inner.session_id)
            .await?
            .1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_protocol::{DeviceId, FileSystemAction, TenantId, TextEncoding};

    #[tokio::test]
    async fn bulk_destination_partial_summary_and_owner_survive_history_projection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.sqlite3");
        let queue = super::super::TransferQueue::open(&path, "owner".into(), "guest".into())
            .await
            .unwrap();
        let store = RuntimeStore::open(&path).await.unwrap();
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        };
        let code = "123456789".parse().unwrap();
        let request = FileSystemRequest {
            execution: Default::default(),
            request_id: RequestId::new(),
            path: "/source".into(),
            operation: FileSystemAction::Move {
                destination: "/target".into(),
                recursive: true,
                overwrite: false,
                limits: pab_protocol::FileOperationLimits::default(),
            },
            payload_size: 0,
            payload_sha256: None,
        };
        store
            .accept_filesystem(&request, device, Some(code), "guest", "owner")
            .await
            .unwrap();
        assert_eq!(queue.active_for_device(code).await.unwrap(), 1);
        assert_eq!(
            queue.active_filesystems().await.unwrap(),
            vec![(request.request_id, code)]
        );
        let mut reply = FileSystemReply::pending(&request);
        reply.state = "cancelled".into();
        reply.mutation = Some(pab_protocol::FileMutationSummary {
            phase: "removing_source".into(),
            total_entries: 4,
            processed_entries: 3,
            published_entries: 2,
            deleted_entries: 1,
            partial: true,
            source_removed: false,
            ..Default::default()
        });
        store.save_filesystem_reply(&reply).await.unwrap();
        let row = store.operations().await.unwrap().remove(0);
        assert_eq!(row.source, "/source");
        assert_eq!(row.destination, "/target");
        assert_eq!(row.kind, "file_move");
        assert_eq!(row.filesystem_mutation, reply.mutation);
        assert!(
            serde_json::to_value(&row)
                .unwrap()
                .get("filesystem_mutation")
                .is_none()
        ); // No item details in presence.
        assert_eq!(queue.active_for_device(code).await.unwrap(), 0);
        let page = queue
            .operation_page(Some(code), None, None, 20)
            .await
            .unwrap();
        assert_eq!(page[0]["destination"], "/target");
        reply.state = "running".into();
        reply.mutation = None;
        store.save_filesystem_reply(&reply).await.unwrap();
        let store = RuntimeStore::open(&path).await.unwrap();
        let (_, saved) = store
            .filesystem_record(request.request_id, Some(code), "guest", "owner")
            .await
            .unwrap();
        assert_eq!(saved.state, "cancelled");
        assert!(saved.mutation.unwrap().partial);
        store.start_session("other").await.unwrap();
        assert!(
            !store
                .owns_filesystem(request.request_id, "other")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn hash_progress_cancel_and_terminal_are_monotonic_and_session_owned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.sqlite3");
        let queue =
            super::super::TransferQueue::open(&path, "owner".to_owned(), "guest".to_owned())
                .await
                .unwrap();
        let store = RuntimeStore::open(&path).await.unwrap();
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        };
        let code = "123456789".parse().unwrap();
        let request = FileSystemRequest {
            execution: Default::default(),
            request_id: RequestId::new(),
            path: "/tmp/hash.bin".to_owned(),
            operation: FileSystemAction::Hash,
            payload_size: 0,
            payload_sha256: None,
        };
        store
            .accept_filesystem(&request, device, Some(code), "guest", "owner")
            .await
            .unwrap();
        let mut reply = FileSystemReply::pending(&request);
        reply.execution_context = Some(serde_json::from_value(serde_json::json!({
            "os_family":"linux","os_name":"Linux","os_version":"test","architecture":"x86_64","execution_scope":"native","path_style":"posix","interpreter":null,"cwd":"/home/fixture","environment_revision":"user-v1:test",
            "identity":{"mode":"user","account_id":"uid:23001","account_name":"fixture","home":"/home/fixture","primary_group":23001,"session_id":null,"logon_id":null,"environment_source":"native_account"}
        })).unwrap());

        reply.progress = Some(pab_protocol::FileHashProgress {
            completed_bytes: 256,
            total_bytes: 1024,
            updated_at_unix_ms: 200,
        });
        store.save_filesystem_reply(&reply).await.unwrap();
        for erased in [false, true] {
            let mut altered = reply.clone();
            if erased {
                altered.execution_context = None;
            } else {
                altered
                    .execution_context
                    .as_mut()
                    .unwrap()
                    .identity
                    .as_mut()
                    .unwrap()
                    .account_id = "uid:23002".into();
            }
            assert!(matches!(
                store.save_filesystem_reply(&altered).await,
                Err(RuntimeStoreError::SnapshotIdentityMismatch)
            ));
        }

        reply.state = "cancel_requested".to_owned();
        store.save_filesystem_reply(&reply).await.unwrap();
        reply.state = "running".to_owned();
        reply.progress = Some(pab_protocol::FileHashProgress {
            completed_bytes: 1,
            total_bytes: 10,
            updated_at_unix_ms: 100,
        });
        store.save_filesystem_reply(&reply).await.unwrap();
        let cached = queue
            .filesystem_record(request.request_id, code)
            .await
            .unwrap()
            .1;
        assert_eq!(cached.state, "cancel_requested");
        assert_eq!(cached.execution_context, reply.execution_context);
        assert_eq!(cached.progress.unwrap().completed_bytes, 256);
        let page = queue
            .operation_page(Some(code), None, None, 20)
            .await
            .unwrap();
        assert_eq!(page[0]["offset"], 256);
        assert_eq!(page[0]["size"], 1024);
        assert_eq!(queue.active_for_device(code).await.unwrap(), 1);
        assert_eq!(
            queue.active_filesystems().await.unwrap(),
            vec![(request.request_id, code)]
        );
        reply.state = "cancelled".to_owned();
        store.save_filesystem_reply(&reply).await.unwrap();
        reply.state = "running".to_owned();
        store.save_filesystem_reply(&reply).await.unwrap();
        assert_eq!(
            queue
                .filesystem_record(request.request_id, code)
                .await
                .unwrap()
                .1
                .state,
            "cancelled"
        );
        assert_eq!(queue.active_for_device(code).await.unwrap(), 0);
        assert!(queue.active_filesystems().await.unwrap().is_empty());
        store.stop_session("owner").await.unwrap();
        store.start_session("new").await.unwrap();
        assert!(
            store
                .filesystem_record(request.request_id, Some(code), "guest", "new")
                .await
                .is_ok()
        );
        assert!(
            store
                .owns_filesystem(request.request_id, "owner")
                .await
                .unwrap()
        );
        assert!(
            !store
                .owns_filesystem(request.request_id, "new")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn pending_filesystem_cap_rejects_new_requests_but_allows_original_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = RuntimeStore::open(&dir.path().join("bridge.sqlite3"))
            .await
            .unwrap();
        store.start_session("owner").await.unwrap();
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        };
        let mut request = FileSystemRequest {
            execution: Default::default(),
            request_id: RequestId::new(),
            path: "/tmp/hash.bin".to_owned(),
            operation: FileSystemAction::Hash,
            payload_size: 0,
            payload_sha256: None,
        };
        for _ in 0..32 {
            request.request_id = RequestId::new();
            assert!(
                store
                    .accept_filesystem(&request, device, None, "guest", "owner")
                    .await
                    .unwrap()
            );
        }
        assert!(
            !store
                .accept_filesystem(&request, device, None, "guest", "owner")
                .await
                .unwrap()
        );
        let prior = request.clone();
        request.request_id = RequestId::new();
        assert!(matches!(
            store
                .accept_filesystem(&request, device, None, "guest", "owner")
                .await,
            Err(RuntimeStoreError::FilesystemBusy)
        ));
        let mut reply = FileSystemReply::pending(&prior);
        reply.state = "failed".to_owned();
        store.save_filesystem_reply(&reply).await.unwrap();
        assert!(
            store
                .accept_filesystem(&request, device, None, "guest", "owner")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn dedup_owner_device_history_and_terminal_first_wins() {
        let dir = tempfile::tempdir().unwrap();
        let store = RuntimeStore::open(&dir.path().join("bridge.sqlite3"))
            .await
            .unwrap();
        store.start_session("first").await.unwrap();
        store.start_session("second").await.unwrap();
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        };
        let code = "123456789".parse().unwrap();
        let request = FileSystemRequest {
            execution: Default::default(),
            request_id: RequestId::new(),
            path: "/tmp/data.txt".to_owned(),
            operation: FileSystemAction::Write {
                encoding: TextEncoding::Utf8,
                overwrite: false,
                expected_hash: None,
            },
            payload_size: 0,
            payload_sha256: Some(format!("{:x}", Sha256::digest(b""))),
        };
        let (a, b) = tokio::join!(
            store.accept_filesystem(&request, device, Some(code), "guest", "first"),
            store.accept_filesystem(&request, device, Some(code), "guest", "first")
        );
        assert_ne!(a.unwrap(), b.unwrap());
        assert!(
            store
                .filesystem_record(request.request_id, Some(code), "guest", "second")
                .await
                .is_err()
        );
        assert!(
            store
                .filesystem_record(
                    request.request_id,
                    Some("987654321".parse().unwrap()),
                    "guest",
                    "first"
                )
                .await
                .is_err()
        );
        assert!(
            store
                .filesystem_record(request.request_id, Some(code), "another_actor", "first")
                .await
                .is_err()
        );
        let mut changed = request.clone();
        changed.path = "/tmp/other.txt".to_owned();
        assert!(matches!(
            store
                .accept_filesystem(&changed, device, Some(code), "guest", "first")
                .await,
            Err(RuntimeStoreError::RequestConflict)
        ));
        let mut reply = FileSystemReply::pending(&request);
        reply.state = "completed".to_owned();
        store.save_filesystem_reply(&reply).await.unwrap();
        reply.state = "unconfirmed".to_owned();
        store.save_filesystem_reply(&reply).await.unwrap();
        assert_eq!(
            store
                .filesystem_record(request.request_id, Some(code), "guest", "first")
                .await
                .unwrap()
                .1
                .state,
            "completed"
        );
        let operations = store.operations().await.unwrap();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].kind, "file_write");
        let queue = super::super::TransferQueue::open(
            &dir.path().join("bridge.sqlite3"),
            "first".to_owned(),
            "guest".to_owned(),
        )
        .await
        .unwrap();
        let page = queue
            .operation_page(Some(code), None, None, 20)
            .await
            .unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0]["kind"], "file_write");
        assert_eq!(queue.active_for_device(code).await.unwrap(), 0);
        store.stop_session("first").await.unwrap();
        assert_eq!(
            store
                .filesystem_record(request.request_id, Some(code), "guest", "second")
                .await
                .unwrap()
                .1
                .state,
            "completed"
        );
    }

    #[tokio::test]
    async fn directory_history_retains_page_and_counts_active_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.sqlite3");
        let queue = super::super::TransferQueue::open(&path, "owner".into(), "guest".into())
            .await
            .unwrap();
        let store = RuntimeStore::open(&path).await.unwrap();
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        };
        let code = "123456789".parse().unwrap();
        let request = FileSystemRequest {
            execution: pab_protocol::ExecutionSelection::User {
                context_ref: pab_protocol::ExecutionContextRef::new(),
            },
            request_id: RequestId::new(),
            path: "/tmp".into(),
            operation: FileSystemAction::ListDirectory {
                after: None,
                limit: 1,
            },
            payload_size: 0,
            payload_sha256: None,
        };
        store
            .accept_filesystem(&request, device, Some(code), "guest", "owner")
            .await
            .unwrap();
        assert_eq!(queue.active_for_device(code).await.unwrap(), 1);
        let mut reply = FileSystemReply::pending(&request);
        reply.state = "completed".into();
        reply.directory = Some(pab_protocol::DirectoryPage {
            request_id: request.request_id,
            path: request.path.clone(),
            execution_context: None,
            entries: vec![pab_protocol::DirectoryEntry {
                name: "a.txt".into(),
                kind: pab_protocol::DirectoryEntryKind::File,
                size: Some(3),
            }],
            next_after: Some("a.txt".into()),
        });
        store.save_filesystem_reply(&reply).await.unwrap();
        assert_eq!(queue.active_for_device(code).await.unwrap(), 0);
        drop(store);
        let reopened = RuntimeStore::open(&path).await.unwrap();
        assert_eq!(
            reopened
                .filesystem_record(request.request_id, Some(code), "guest", "owner")
                .await
                .unwrap()
                .1
                .directory,
            reply.directory
        );
        let records = reopened.operations().await.unwrap();
        assert_eq!(records[0].kind, "directory");
        assert_eq!((records[0].offset, records[0].size), (1, 1));
    }

    #[tokio::test]
    async fn unresolved_files_block_disconnect_and_are_visible_as_unconfirmed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.sqlite3");
        let queue =
            super::super::TransferQueue::open(&path, "first".to_owned(), "guest".to_owned())
                .await
                .unwrap();
        let store = RuntimeStore::open(&path).await.unwrap();
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        };
        let code = "123456789".parse().unwrap();
        let request = FileSystemRequest {
            execution: Default::default(),
            request_id: RequestId::new(),
            path: "/tmp/read.txt".to_owned(),
            operation: FileSystemAction::Stat {
                follow_symlinks: false,
            },
            payload_size: 0,
            payload_sha256: None,
        };
        store
            .accept_filesystem(&request, device, Some(code), "guest", "first")
            .await
            .unwrap();
        assert_eq!(queue.active_for_device(code).await.unwrap(), 1);
        assert_eq!(
            store.operations().await.unwrap()[0]
                .execution_observation
                .as_deref(),
            Some("unconfirmed")
        );
        assert_eq!(
            queue
                .filesystem_record(request.request_id, code)
                .await
                .unwrap()
                .1
                .state,
            "unconfirmed"
        );
    }
}
