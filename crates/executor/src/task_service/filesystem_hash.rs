use super::{TaskService, TaskServiceError, filesystem::FileError, filesystem_io as io};
use pab_protocol::{FileHashProgress, FileSystemReply, FileSystemRequest, OperatorRef, RequestId};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};
use tokio::{fs, io::AsyncReadExt, sync::watch, task::JoinHandle};

#[cfg(test)]
#[derive(Default)]
pub(super) struct HashTestGate {
    pub after_open: bool,
    pub started: tokio::sync::Notify,
    pub resume: tokio::sync::Notify,
}

pub(super) struct HashJob {
    pub(super) cancel: watch::Sender<bool>,
    pub(super) _task: JoinHandle<()>,
}

impl TaskService {
    pub(super) async fn start_hash(
        &self,
        request: &FileSystemRequest,
        context: pab_protocol::ExecutionContext,
    ) -> Result<FileSystemReply, TaskServiceError> {
        let mut reply = FileSystemReply::pending(request);
        reply.execution_context = Some(context.clone());
        reply.progress = Some(FileHashProgress {
            completed_bytes: 0,
            total_bytes: 0,
            updated_at_unix_ms: now(),
        });
        let permit = match self.hash_slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                reply.state = "failed".to_owned();
                reply.error = Some(
                    FileError::new(
                        "executor_busy",
                        "accept",
                        "Executor has 4 active hash operations; retry with a new request later",
                    )
                    .0,
                );
                self.store.finish_filesystem(&reply).await?;
                return Ok(reply);
            }
        };
        self.store.update_filesystem_progress(&reply).await?;
        let (cancel, mut receiver) = watch::channel(false);
        let worker = self.clone();
        let id = request.request_id;
        let mut jobs = self.hash_jobs.lock().await;
        let task = tokio::spawn(async move {
            let _permit = permit;
            let mut final_reply = reply.clone();
            let mut stop = receiver.clone();
            let engine = worker.file_engine();
            let result = tokio::select! {
                changed = stop.changed() => { let _ = changed; Err(FileError::new("cancelled", "hash", "hash task stopped after cancellation")) },
                result = tokio::time::timeout(Duration::from_secs(30 * 60), engine.hash_file(&mut final_reply, &mut receiver)) => result.unwrap_or_else(|_| Err(FileError::new("time_limit", "hash", "hash exceeded 30 minutes"))),
            };
            match result {
                Ok(()) => final_reply.state = "completed".to_owned(),
                Err(error) => {
                    final_reply.state = if error.0.code == "cancelled" {
                        "cancelled"
                    } else {
                        "failed"
                    }
                    .to_owned();
                    final_reply.error = Some(error.0);
                }
            }
            if let Err(error) = worker.store.finish_filesystem(&final_reply).await {
                tracing::warn!(%id, %error, "failed to persist hash result");
            }
            worker.hash_jobs.lock().await.remove(&id);
        });
        jobs.insert(
            id,
            HashJob {
                cancel,
                _task: task,
            },
        );
        // This acknowledges acceptance, never waits for hashing the file.
        let mut accepted = FileSystemReply::pending(request);
        accepted.execution_context = Some(context);
        accepted.progress = Some(FileHashProgress {
            completed_bytes: 0,
            total_bytes: 0,
            updated_at_unix_ms: now(),
        });
        Ok(accepted)
    }

    pub(super) async fn cancel_hash(
        &self,
        actor: OperatorRef,
        id: RequestId,
    ) -> Result<FileSystemReply, TaskServiceError> {
        let mut reply = self.store.get_filesystem(actor, id).await?;
        if pab_protocol::cancellable_filesystem_kind(&reply.kind) && reply.kind != "file_hash" {
            return self.cancel_bulk(actor, id).await;
        }
        if reply.kind != "file_hash" {
            return Err(TaskServiceError::InvalidRequest(
                "only hash filesystem operations support cancellation",
            ));
        }
        if !matches!(reply.state.as_str(), "running" | "cancel_requested") {
            return Ok(reply);
        }
        let jobs = self.hash_jobs.lock().await;
        let Some(job) = jobs.get(&id) else {
            return Err(TaskServiceError::InvalidRequest(
                "hash worker is not registered; query or retry cancellation using the original ID",
            ));
        };
        reply.state = "cancel_requested".to_owned();
        self.store.request_filesystem_cancel(&reply).await?;
        let _ = job.cancel.send(true);
        // Read back a competing terminal result if the worker already finished.
        self.store
            .get_filesystem(actor, id)
            .await
            .map_err(Into::into)
    }
}

impl super::filesystem_engine::FileEngine {
    pub(crate) async fn hash_file(
        &self,
        reply: &mut FileSystemReply,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(), FileError> {
        #[cfg(test)]
        let test_gate = self.hash_test_gate.lock().await.clone();
        #[cfg(test)]
        if let Some(gate) = &test_gate
            && !gate.after_open
        {
            gate.started.notify_one();
            gate.resume.notified().await;
        }
        let path = Path::new(&reply.path);
        cancelled(cancel)?;
        io::no_links(path, false).await?;
        let inspected = fs::symlink_metadata(path)
            .await
            .map_err(|e| io::io_error("stat", e))?;
        if !inspected.is_file() {
            return Err(FileError::new(
                "not_regular_file",
                "hash",
                "hash requires an ordinary file",
            ));
        }
        let mut file = fs::File::open(path)
            .await
            .map_err(|e| io::io_error("open", e))?;
        let before = file.metadata().await.map_err(|e| io::io_error("stat", e))?;
        if !before.is_file() {
            return Err(FileError::new(
                "not_regular_file",
                "hash",
                "hash requires an ordinary file",
            ));
        }
        reply.metadata = Some(io::metadata(&before));
        reply.progress.as_mut().unwrap().total_bytes = before.len();
        self.store
            .update_filesystem_progress(reply)
            .await
            .map_err(|e| FileError::new("record_error", "hash", e.to_string()))?;
        #[cfg(test)]
        if let Some(gate) = &test_gate
            && gate.after_open
        {
            gate.started.notify_one();
            gate.resume.notified().await;
        }
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 256 * 1024];
        let mut bytes = 0u64;
        let mut last_record = tokio::time::Instant::now();
        loop {
            cancelled(cancel)?;
            let read = tokio::select! {
                changed = cancel.changed() => { let _ = changed; return Err(FileError::new("cancelled", "hash", "hash stopped after cancellation")); },
                read = tokio::time::timeout(Duration::from_secs(30), file.read(&mut buffer)) => read.map_err(|_| FileError::new("read_timeout", "hash", "file read exceeded 30 seconds"))?.map_err(|e| io::io_error("read", e))?,
            };
            if read == 0 {
                break;
            }
            bytes += read as u64;
            if bytes > before.len() {
                return Err(FileError::new(
                    "version_conflict",
                    "hash",
                    "file grew during hashing",
                ));
            }
            hasher.update(&buffer[..read]);
            reply.progress.as_mut().unwrap().completed_bytes = bytes;
            if last_record.elapsed() >= Duration::from_millis(250) {
                reply.progress.as_mut().unwrap().updated_at_unix_ms = now();
                self.store
                    .update_filesystem_progress(reply)
                    .await
                    .map_err(|e| FileError::new("record_error", "hash", e.to_string()))?;
                last_record = tokio::time::Instant::now();
            }
        }
        cancelled(cancel)?;
        let after = file.metadata().await.map_err(|e| io::io_error("stat", e))?;
        io::no_links(path, false).await?;
        let named = fs::metadata(path)
            .await
            .map_err(|e| io::io_error("stat", e))?;
        if bytes != before.len()
            || after.len() != before.len()
            || after.modified().ok() != before.modified().ok()
            || named.len() != before.len()
            || named.modified().ok() != before.modified().ok()
            || !same_file(&before, &named)
        {
            return Err(FileError::new(
                "version_conflict",
                "hash",
                "file changed during hashing; retry with a new request",
            ));
        }
        reply.metadata.as_mut().unwrap().sha256 = Some(hex::encode(hasher.finalize()));
        reply.progress.as_mut().unwrap().updated_at_unix_ms = now();
        Ok(())
    }
}

fn cancelled(cancel: &watch::Receiver<bool>) -> Result<(), FileError> {
    if *cancel.borrow() {
        Err(FileError::new(
            "cancelled",
            "hash",
            "hash stopped after cancellation",
        ))
    } else {
        Ok(())
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().min(i64::MAX as u128) as i64)
}

fn same_file(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.dev() == after.dev() && before.ino() == after.ino()
    }
    #[cfg(not(unix))]
    {
        before.created().ok() == after.created().ok()
    }
}
