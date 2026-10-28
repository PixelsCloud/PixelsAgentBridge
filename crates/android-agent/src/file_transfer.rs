//! Existing transfer v2 wire format, backed by Android's granted storage adapter.
//! Durable publication boundaries prevent implicit replay after a lost response.
use super::*;
use pab_transport::{PabBiStream, PabConnectionError};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

type Error = Box<dyn std::error::Error + Send + Sync>;
const MAX_FILE: u64 = 1024 * 1024 * 1024;

struct TransferLease {
    active: Arc<std::sync::Mutex<HashSet<(String, RequestId)>>>,
    key: (String, RequestId),
    _slot: tokio::sync::OwnedSemaphorePermit,
}
impl Drop for TransferLease {
    fn drop(&mut self) {
        self.active.lock().unwrap().remove(&self.key);
    }
}

pub(super) async fn initialize(pool: &SqlitePool) -> Result<(), &'static str> {
    sqlx::query("CREATE TABLE IF NOT EXISTS file_transfers(owner TEXT NOT NULL,id TEXT NOT NULL,digest TEXT NOT NULL,request TEXT NOT NULL,snapshot TEXT NOT NULL,stage_id TEXT NOT NULL,resumed_by TEXT,PRIMARY KEY(owner,id)) WITHOUT ROWID")
        .execute(pool).await.map_err(|_| "file_journal_unavailable")?;
    let columns = sqlx::query("PRAGMA table_info(file_transfers)")
        .fetch_all(pool)
        .await
        .map_err(|_| "file_journal_unavailable")?;
    if !columns
        .iter()
        .any(|row| row.get::<String, _>("name") == "resumed_by")
    {
        sqlx::query("ALTER TABLE file_transfers ADD COLUMN resumed_by TEXT")
            .execute(pool)
            .await
            .map_err(|_| "file_journal_unavailable")?;
    }
    sqlx::query("UPDATE file_transfers SET snapshot=json_set(snapshot,'$.state',CASE WHEN json_extract(snapshot,'$.state')='publishing' THEN 'unconfirmed' ELSE 'interrupted' END,'$.message','Android process stopped; query before explicitly resuming') WHERE json_extract(snapshot,'$.state') IN ('running','publishing')")
        .execute(pool).await.map_err(|_| "file_journal_unavailable")?;
    Ok(())
}

impl FileService {
    pub async fn transfer_snapshot(
        &self,
        owner: &str,
        id: RequestId,
    ) -> Result<Option<TransferSnapshot>, &'static str> {
        let body = sqlx::query_scalar::<_, String>(
            "SELECT snapshot FROM file_transfers WHERE owner=? AND id=?",
        )
        .bind(owner)
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| "Transfer outcome could not be read; do not replay")?;
        let Some(body) = body else {
            return Ok(None);
        };
        let mut snapshot: TransferSnapshot = serde_json::from_str(&body)
            .map_err(|_| "Transfer outcome could not be decoded; do not replay")?;
        let active = self
            .active_transfers
            .lock()
            .unwrap()
            .contains(&(owner.into(), id));
        if !active && matches!(snapshot.state.as_str(), "running" | "publishing") {
            snapshot.state = if snapshot.state == "publishing" {
                "unconfirmed"
            } else {
                "interrupted"
            }
            .into();
            snapshot.message =
                Some("Original transfer stream ended; query before explicitly resuming".into());
            snapshot.finished_at_unix_ms = Some(crate::peer::now_ms());
            if self.save_transfer(owner, &snapshot).await.is_err() {
                return Err("Transfer interruption could not be recorded; do not replay");
            }
        }
        Ok(Some(snapshot))
    }

    async fn save_transfer(&self, owner: &str, snapshot: &TransferSnapshot) -> Result<(), Error> {
        sqlx::query("UPDATE file_transfers SET snapshot=? WHERE owner=? AND id=?")
            .bind(serde_json::to_string(snapshot)?)
            .bind(owner)
            .bind(snapshot.request_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn transfer(
        &self,
        owner: &str,
        operator: OperatorRef,
        user: Option<UserAttribution>,
        context: ExecutionContext,
        request: FileTransferRequest,
        stream: &mut PabBiStream,
    ) -> Result<(), PabConnectionError> {
        let timeout = Duration::from_secs(60);
        match self
            .accept_transfer(owner, operator, user, context, &request)
            .await
        {
            Err(error) => {
                stream
                    .send_json(
                        &DeviceTaskResponse::Error {
                            code: DeviceTaskErrorCode::InvalidRequest,
                            message: error.to_string(),
                        },
                        timeout,
                    )
                    .await
            }
            Ok((mut snapshot, stage_id, lease)) => {
                let Some(_lease) = lease else {
                    return stream
                        .send_json(&DeviceTaskResponse::Transfer { snapshot }, timeout)
                        .await;
                };
                // From here the original ID exists even if the peer closes before accepting bytes.
                let result = async {
                    if let FileTransferOperation::Upload {
                        path, overwrite, ..
                    } = &request.operation
                    {
                        self.io(json!({"action":"check_upload","path":path,"overwrite":overwrite}))
                            .await?;
                    }
                    stream
                        .send_frame_json(
                            &DeviceTaskResponse::TransferAccepted {
                                snapshot: snapshot.clone(),
                            },
                            timeout,
                        )
                        .await?;
                    let _guard = self.execution.lock().await;
                    self.transfer_bytes(owner, &request, &mut snapshot, stage_id, stream)
                        .await
                }
                .await;
                if let Err(error) = result {
                    if snapshot.state != "completed" {
                        snapshot.state = if snapshot.state == "publishing" {
                            "unconfirmed"
                        } else {
                            "interrupted"
                        }
                        .into();
                        snapshot.message = Some(error.to_string().chars().take(1024).collect());
                        snapshot.finished_at_unix_ms = Some(crate::peer::now_ms());
                        let _ = self.save_transfer(owner, &snapshot).await;
                    }
                    let _ = stream
                        .send_json(
                            &DeviceTaskResponse::Error {
                                code: DeviceTaskErrorCode::Internal,
                                message: error.to_string().chars().take(1024).collect(),
                            },
                            timeout,
                        )
                        .await;
                }
                Ok(())
            }
        }
    }

    async fn accept_transfer(
        &self,
        owner: &str,
        operator: OperatorRef,
        user: Option<UserAttribution>,
        context: ExecutionContext,
        request: &FileTransferRequest,
    ) -> Result<(TransferSnapshot, Uuid, Option<TransferLease>), Error> {
        request.validate()?;
        if request.expected_execution.is_some() || request.source_constraint.is_some() || request.metadata.is_some() {
            return Err("Native file execution preconditions require an updated Android storage adapter".into());
        }
        if request.native_path.as_ref().is_some_and(|path| !matches!(path, pab_protocol::NativeFilePath::Utf8(value) if value == request.operation.path())) {
            return Err("Android storage supports UTF-8 native paths only".into());
        }
        if !request.execution.is_service() {
            return Err(
                "Android files execute as the app UID; alternate user execution is unsupported"
                    .into(),
            );
        }
        if let FileTransferOperation::Upload { size, .. } = &request.operation
            && *size > MAX_FILE
        {
            return Err("Android transfer limit is 1 GiB per file".into());
        }
        let _guard = self.acceptance.lock().await;
        let key = blake3::keyed_hash(&self.digest_key, &serde_json::to_vec(request)?)
            .to_hex()
            .to_string();
        if let Some(row) = sqlx::query(
            "SELECT digest,snapshot,stage_id FROM file_transfers WHERE owner=? AND id=?",
        )
        .bind(owner)
        .bind(request.request_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        {
            if row.get::<String, _>("digest") != key {
                return Err("request_conflict: transfer parameters differ".into());
            }
            let observed = self
                .transfer_snapshot(owner, request.request_id)
                .await?
                .ok_or("Transfer outcome unavailable")?;
            return Ok((observed, row.get::<String, _>("stage_id").parse()?, None));
        }
        if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM file_transfers")
            .fetch_one(&self.pool)
            .await?
            >= 10000
        {
            return Err(
                "Transfer journal capacity reached; existing outcomes are preserved".into(),
            );
        }
        let mut stage_id = Uuid::new_v4();
        if sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM file_operations WHERE owner=? AND id=?",
        )
        .bind(owner)
        .bind(request.request_id.to_string())
        .fetch_one(&self.pool)
        .await?
            > 0
        {
            return Err("request_conflict: ID already belongs to a filesystem operation".into());
        }
        let slot = Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|_| "File service is busy")?;
        let lease = TransferLease {
            active: self.active_transfers.clone(),
            key: (owner.into(), request.request_id),
            _slot: slot,
        };
        self.active_transfers
            .lock()
            .unwrap()
            .insert(lease.key.clone());
        let (size, hash) = match &request.operation {
            FileTransferOperation::Upload { size, sha256, .. } => (*size, Some(sha256.clone())),
            FileTransferOperation::Download { .. } => (0, None),
        };
        if let Some(original) = request.resume_from {
            let row = sqlx::query(
                "SELECT request,snapshot,stage_id,resumed_by FROM file_transfers WHERE owner=? AND id=?",
            )
            .bind(owner)
            .bind(original.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or("Resume source was not found for this caller")?;
            if row.get::<Option<String>, _>("resumed_by").is_some() {
                return Err("Resume source already belongs to another explicit attempt".into());
            }
            let old_request: FileTransferRequest = serde_json::from_str(row.get("request"))?;
            let old = self
                .transfer_snapshot(owner, original)
                .await?
                .ok_or("Resume source outcome unavailable")?;
            // Sender completion is not evidence of receiver-side publication. A
            // cancelled local download may still need its verified prefix even
            // when every byte was already buffered by the transport. This is a
            // new, explicit read bound below to the original path and full hash.
            let completed_download = old.state == "completed"
                && matches!(
                    old_request.operation,
                    FileTransferOperation::Download { .. }
                )
                && matches!(request.operation, FileTransferOperation::Download { .. });
            if !(matches!(old.state.as_str(), "interrupted" | "failed" | "cancelled")
                || completed_download)
                || old.published == Some(true)
            {
                return Err(
                    "Cannot resume a running/unconfirmed transfer or a completed upload".into(),
                );
            }
            let compatible = old_request.native_path == request.native_path
                && match (&old_request.operation, &request.operation) {
                    (
                        FileTransferOperation::Upload {
                            path: a,
                            size: b,
                            sha256: c,
                            overwrite: d,
                        },
                        FileTransferOperation::Upload {
                            path: w,
                            size: x,
                            sha256: y,
                            overwrite: z,
                        },
                    ) => a == w && b == x && c == y && d == z,
                    (
                        FileTransferOperation::Download { path: a, .. },
                        FileTransferOperation::Download {
                            path: b,
                            expected_sha256: Some(expected),
                            ..
                        },
                    ) => a == b && old.sha256.as_ref() == Some(expected),
                    _ => false,
                };
            if !compatible {
                return Err("Resume source path, size or hash differs".into());
            }
            if matches!(request.operation, FileTransferOperation::Upload { .. }) {
                // The journal transaction transfers the right to continue this
                // private prefix exactly once. Keeping its path avoids losing
                // resumable bytes between a filesystem rename and a DB commit.
                stage_id = row.get::<String, _>("stage_id").parse()?;
            }
        }
        let snapshot = TransferSnapshot {
            metadata_result:None,
            initiating_user: user,
            execution_context: Some(context),
            request_id: request.request_id,
            initiated_by: operator,
            direction: request.operation.direction().into(),
            path: request.operation.path().into(),
            state: "running".into(),
            offset: 0,
            size,
            sha256: hash,
            finished_at_unix_ms: None,
            message: None,
            published: Some(false),
        };
        let mut transaction = self.pool.begin().await?;
        if let Some(original) = request.resume_from {
            let changed=sqlx::query("UPDATE file_transfers SET resumed_by=? WHERE owner=? AND id=? AND resumed_by IS NULL")
                .bind(request.request_id.to_string()).bind(owner).bind(original.to_string()).execute(&mut *transaction).await?;
            if changed.rows_affected() != 1 {
                return Err("Resume source was already consumed".into());
            }
        }
        sqlx::query("INSERT INTO file_transfers(owner,id,digest,request,snapshot,stage_id) VALUES(?,?,?,?,?,?)")
            .bind(owner).bind(request.request_id.to_string()).bind(key).bind(serde_json::to_string(request)?)
            .bind(serde_json::to_string(&snapshot)?).bind(stage_id.to_string()).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok((snapshot, stage_id, Some(lease)))
    }

    async fn transfer_bytes(
        &self,
        owner: &str,
        request: &FileTransferRequest,
        snapshot: &mut TransferSnapshot,
        stage_id: Uuid,
        stream: &mut PabBiStream,
    ) -> Result<(), Error> {
        let timeout = Duration::from_secs(60);
        match &request.operation {
            FileTransferOperation::Upload {
                path,
                size,
                sha256,
                overwrite,
            } => {
                let stage = self.staging.join(format!("{stage_id}.part"));
                let retained = tokio::fs::metadata(&stage)
                    .await
                    .map(|meta| meta.len())
                    .unwrap_or(0);
                self.io(json!({"action":"check_staging","path":"/","additional_bytes":size.saturating_sub(retained)})).await?;
                let mut file = tokio::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(&stage)
                    .await?;
                let mut offset = file.metadata().await?.len();
                if offset > *size {
                    return Err("Staged prefix exceeds declared file size".into());
                }
                file.seek(std::io::SeekFrom::Start(offset)).await?;
                snapshot.offset = offset;
                self.save_transfer(owner, snapshot).await?;
                stream
                    .send_frame_json(
                        &DeviceTaskResponse::FileReady {
                            size: *size,
                            offset,
                            sha256: sha256.clone(),
                        },
                        timeout,
                    )
                    .await?;
                while offset < *size {
                    let bytes = stream.receive_binary_frame(timeout).await?;
                    if bytes.is_empty()
                        || bytes.len() > 65536
                        || offset + bytes.len() as u64 > *size
                    {
                        return Err("Invalid upload frame size".into());
                    }
                    file.write_all(&bytes).await?;
                    file.sync_data().await?;
                    offset += bytes.len() as u64;
                    snapshot.offset = offset;
                    self.save_transfer(owner, snapshot).await?;
                    stream
                        .send_frame_json(&DeviceTaskResponse::FileProgress { offset }, timeout)
                        .await?;
                }
                file.sync_all().await?;
                drop(file);
                // Transfer v2 is delimited by declared size; existing clients await
                // FileComplete without half-closing their request stream.
                if hash_file(&stage).await? != *sha256 {
                    return Err("Upload SHA-256 mismatch; file was not published".into());
                }
                snapshot.state = "publishing".into();
                snapshot.published = None;
                self.save_transfer(owner, snapshot).await?;
                self.io(json!({"action":"publish","path":path,"staging_id":stage_id,"overwrite":overwrite})).await?;
                snapshot.state = "completed".into();
                snapshot.published = Some(true);
                snapshot.finished_at_unix_ms = Some(crate::peer::now_ms());
                self.save_transfer(owner, snapshot).await?;
                let _ = tokio::fs::remove_file(stage).await;
                stream
                    .send_json(
                        &DeviceTaskResponse::FileComplete {
                            size: *size,
                            sha256: sha256.clone(),
                        },
                        timeout,
                    )
                    .await?;
            }
            FileTransferOperation::Download {
                path,
                offset,
                expected_sha256,
            } => {
                let (stage, value) = self.snapshot(path, MAX_FILE).await?;
                let size = value["size"].as_u64().ok_or("Missing snapshot size")?;
                let hash = value["sha256"].as_str().ok_or("Missing snapshot hash")?;
                if !valid_file_hash(hash)
                    || *offset > size
                    || expected_sha256
                        .as_ref()
                        .is_some_and(|wanted| wanted != hash)
                {
                    return Err("Download offset or expected hash differs".into());
                }
                if hash_file(&stage.0).await? != hash {
                    return Err("Snapshot SHA-256 mismatch".into());
                }
                snapshot.size = size;
                snapshot.offset = *offset;
                snapshot.sha256 = Some(hash.into());
                self.save_transfer(owner, snapshot).await?;
                let mut file = tokio::fs::File::open(&stage.0).await?;
                file.seek(std::io::SeekFrom::Start(*offset)).await?;
                stream
                    .send_frame_json(
                        &DeviceTaskResponse::FileReady {
                            size,
                            offset: *offset,
                            sha256: hash.into(),
                        },
                        timeout,
                    )
                    .await?;
                let mut buffer = vec![0; 65536];
                while snapshot.offset < size {
                    let limit = (size - snapshot.offset).min(buffer.len() as u64) as usize;
                    let count = file.read(&mut buffer[..limit]).await?;
                    if count == 0 {
                        return Err("Download snapshot became shorter".into());
                    }
                    stream.send_binary_frame(&buffer[..count], timeout).await?;
                    snapshot.offset += count as u64;
                    self.save_transfer(owner, snapshot).await?;
                }
                snapshot.state = "completed".into();
                snapshot.finished_at_unix_ms = Some(crate::peer::now_ms());
                self.save_transfer(owner, snapshot).await?;
                stream
                    .send_json(
                        &DeviceTaskResponse::FileComplete {
                            size,
                            sha256: hash.into(),
                        },
                        timeout,
                    )
                    .await?;
            }
        }
        Ok(())
    }
}

async fn hash_file(path: &Path) -> Result<String, Error> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 65536];
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
#[path = "file_transfer_tests.rs"]
mod tests;
