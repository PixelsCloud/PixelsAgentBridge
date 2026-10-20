//! Hash a bounded SDK snapshot, with durable progress and cooperative cancellation.
use super::*;
use tokio::io::AsyncReadExt;

impl FileService {
    pub(super) async fn hash(
        &self,
        owner: &str,
        request: &FileSystemRequest,
        reply: &mut FileSystemReply,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, FileError> {
        cancelled(cancel)?;
        // SDK snapshots read grants again for every operation. They never expose native paths.
        // A cancellation waits for an in-flight provider call rather than claiming it stopped.
        let (stage, value) = self.snapshot(&request.path, 1024 * 1024 * 1024).await?;
        cancelled(cancel)?;
        let mut file = tokio::fs::File::open(&stage.0).await.map_err(io_error)?;
        let total = file.metadata().await.map_err(io_error)?.len();
        let mut progress = FileHashProgress {
            completed_bytes: 0,
            total_bytes: total,
            updated_at_unix_ms: crate::peer::now_ms(),
        };
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 65536];
        let mut updated = tokio::time::Instant::now();
        loop {
            cancelled(cancel)?;
            let size = file.read(&mut buffer).await.map_err(io_error)?;
            if size == 0 {
                break;
            }
            hasher.update(&buffer[..size]);
            progress.completed_bytes += size as u64;
            if updated.elapsed() >= Duration::from_millis(250) {
                progress.updated_at_unix_ms = crate::peer::now_ms();
                self.hash_progress(owner, request.request_id, &progress)
                    .await?;
                updated = tokio::time::Instant::now();
            }
        }
        cancelled(cancel)?;
        if progress.completed_bytes != total {
            return Err(FileError::new(
                "version_conflict",
                "hash",
                "Snapshot size changed",
            ));
        }
        let hash = format!("{:x}", hasher.finalize());
        check_hash(value["sha256"].as_str(), &hash)?;
        let mut meta = metadata(&value["metadata"]);
        meta.size = total;
        meta.sha256 = Some(hash);
        reply.metadata = Some(meta);
        progress.updated_at_unix_ms = crate::peer::now_ms();
        reply.progress = Some(progress);
        Ok(Vec::new())
    }

    async fn hash_progress(
        &self,
        owner: &str,
        id: RequestId,
        progress: &FileHashProgress,
    ) -> Result<(), FileError> {
        sqlx::query("UPDATE file_operations SET reply=json_set(reply,'$.progress',json(?)) WHERE owner=? AND id=? AND json_extract(reply,'$.state') IN ('running','cancel_requested')")
            .bind(serde_json::to_string(progress).unwrap()).bind(owner).bind(id.to_string())
            .execute(&self.pool).await.map_err(|_| FileError::new("journal_unavailable", "hash", "Hash progress could not be stored"))?;
        Ok(())
    }
}
