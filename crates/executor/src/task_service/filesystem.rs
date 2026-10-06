use super::{
    TaskService, TaskServiceError,
    filesystem_io::{
        check_hash, digest, io_error, is_link, load, metadata, no_links, validate_path,
    },
    filesystem_text as text,
};
use pab_protocol::{
    DeviceTaskResponse, FileSystemAction, FileSystemError, FileSystemReply, FileSystemRequest,
    OperatorRef, RequestId, TextEdit,
};
use pab_transport::PabBiStream;
use std::{path::Path, time::Duration};
use tokio::fs;

#[derive(Debug)]
pub(crate) struct FileError(pub FileSystemError);

impl FileError {
    pub(super) fn new(code: &str, phase: &str, message: impl Into<String>) -> Self {
        Self(FileSystemError {
            code: code.to_owned(),
            phase: phase.to_owned(),
            message: message.into().chars().take(2048).collect(),
        })
    }
}

impl TaskService {
    pub(super) async fn filesystem_stream(
        &self,
        actor: OperatorRef,
        request: FileSystemRequest,
        stream: &mut PabBiStream,
        timeout: Duration,
    ) -> Result<(), TaskServiceError> {
        let _permit = match self.filesystem_slots.try_acquire() {
            Ok(permit) => permit,
            Err(_) => {
                let mut reply = FileSystemReply::pending(&request);
                reply.state = "failed".to_owned();
                reply.error = Some(
                    FileError::new(
                        "executor_busy",
                        "accept",
                        "Executor has 8 active filesystem stream operations; retry later",
                    )
                    .0,
                );
                stream
                    .send_json(
                        &DeviceTaskResponse::FileSystem {
                            reply: Box::new(reply),
                        },
                        timeout,
                    )
                    .await?;
                return Ok(());
            }
        };
        let result = async {
            request
                .validate()
                .map_err(|message| FileError::new("invalid_request", "validate", message))?;
            validate_path(Path::new(&request.path))?;
            Ok::<_, FileError>(())
        }
        .await;
        if let Err(error) = result {
            let mut reply = FileSystemReply::pending(&request);
            reply.state = "failed".to_owned();
            reply.error = Some(error.0);
            stream
                .send_json(
                    &DeviceTaskResponse::FileSystem {
                        reply: Box::new(reply),
                    },
                    timeout,
                )
                .await?;
            return Ok(());
        }
        let mut payload = Vec::with_capacity(request.payload_size as usize);
        while payload.len() < request.payload_size as usize {
            let frame = stream.receive_binary_frame(timeout).await?;
            if frame.is_empty()
                || frame.len() > 64 * 1024
                || payload.len() + frame.len() > request.payload_size as usize
            {
                return Err(TaskServiceError::InvalidRequest(
                    "invalid text payload frame size",
                ));
            }
            payload.extend_from_slice(&frame);
        }
        stream.expect_receive_end(timeout).await?;
        if request.operation.has_payload()
            && request.payload_sha256.as_deref() != Some(digest(&payload).as_str())
        {
            return Err(TaskServiceError::InvalidRequest(
                "text payload hash mismatch",
            ));
        }
        let (reply, bytes) = self.execute_filesystem(actor, &request, &payload).await?;
        stream
            .send_frame_json(
                &DeviceTaskResponse::FileSystem {
                    reply: Box::new(reply),
                },
                timeout,
            )
            .await?;
        for chunk in bytes.chunks(64 * 1024) {
            stream.send_binary_frame(chunk, timeout).await?;
        }
        stream.finish_send(timeout).await?;
        Ok(())
    }

    pub(super) async fn lookup_filesystem(
        &self,
        actor: OperatorRef,
        id: RequestId,
    ) -> Result<FileSystemReply, TaskServiceError> {
        let mut reply = self.store.get_filesystem(actor, id).await?;
        let registered = if reply.kind == "file_hash" {
            self.hash_jobs.lock().await.contains_key(&id)
        } else {
            self.bulk_jobs.lock().await.contains_key(&id)
        };
        if pab_protocol::cancellable_filesystem_kind(&reply.kind)
            && matches!(reply.state.as_str(), "running" | "cancel_requested")
            && !registered
        {
            // Acceptance registration can briefly be in progress, or the final
            // database write may have failed. Neither proves a terminal result.
            // The worker may also have completed since our first database read.
            reply = self.store.get_filesystem(actor, id).await?;
            if matches!(reply.state.as_str(), "running" | "cancel_requested") {
                reply.state = "unconfirmed".to_owned();
                reply.error = Some(FileError::new("worker_unavailable", "observe", "filesystem worker is not registered; query the original operation to confirm its outcome").0);
            }
        }
        let Ok(_permit) = self.filesystem_slots.try_acquire() else {
            return Ok(reply);
        };
        if matches!(reply.kind.as_str(), "file_write" | "file_patch")
            && reply.state == "unconfirmed"
            && let Some(meta) = &reply.metadata
            && let Some(hash) = meta.sha256.as_deref()
        {
            let lock = self.upload_locks.try_acquire(Path::new(&reply.path)).await;
            if let Ok(Some(_guard)) = lock
                && let Ok((bytes, _)) = load(Path::new(&reply.path)).await
                && bytes.len() as u64 == meta.size
                && digest(&bytes) == hash
            {
                reply.state = "completed".to_owned();
                reply.error = None;
                self.store.finish_filesystem(&reply).await?;
            }
        }
        Ok(reply)
    }

    pub(super) async fn execute_filesystem(
        &self,
        actor: OperatorRef,
        request: &FileSystemRequest,
        payload: &[u8],
    ) -> Result<(FileSystemReply, Vec<u8>), TaskServiceError> {
        let fingerprint =
            digest(&serde_json::to_vec(request).map_err(crate::task_store::TaskStoreError::from)?);
        if let Some(mut existing) = self
            .store
            .accept_filesystem(actor, request, &fingerprint)
            .await?
        {
            if existing.kind == "file_read" && existing.state == "completed" {
                existing.state = "failed".to_owned();
                existing.error = Some(
                    FileError::new(
                        "read_request_consumed",
                        "read",
                        "use a new read request and expected_hash for another range",
                    )
                    .0,
                );
            } else if existing.state == "unconfirmed"
                || (pab_protocol::cancellable_filesystem_kind(&existing.kind)
                    && matches!(existing.state.as_str(), "running" | "cancel_requested"))
            {
                existing = self.lookup_filesystem(actor, request.request_id).await?;
            }
            return Ok((existing, Vec::new()));
        }
        if matches!(request.operation, FileSystemAction::Hash) {
            return Ok((self.start_hash(request).await?, Vec::new()));
        }
        if request.operation.is_bulk() {
            return Ok((self.start_bulk(request).await?, Vec::new()));
        }
        let mut reply = FileSystemReply::pending(request);
        let result = self
            .file_engine()
            .prepare_filesystem(request, payload, &mut reply)
            .await;
        let bytes = match result {
            Ok(bytes) => {
                reply.state = "completed".to_owned();
                bytes
            }
            Err(error) => {
                reply.state = "failed".to_owned();
                reply.error = Some(error.0);
                Vec::new()
            }
        };
        self.store.finish_filesystem(&reply).await?;
        Ok((reply, bytes))
    }
}

impl super::filesystem_engine::FileEngine {
    pub(crate) async fn prepare_filesystem(
        &self,
        request: &FileSystemRequest,
        payload: &[u8],
        reply: &mut FileSystemReply,
    ) -> Result<Vec<u8>, FileError> {
        let path = Path::new(&request.path);
        match &request.operation {
            FileSystemAction::Copy { .. }
            | FileSystemAction::Move { .. }
            | FileSystemAction::Delete { .. }
            | FileSystemAction::ArchiveCreate { .. }
            | FileSystemAction::ArchiveExtract { .. } => {
                unreachable!("bulk actions use their asynchronous handler")
            }
            FileSystemAction::Hash => unreachable!("hash uses its asynchronous handler"),
            FileSystemAction::Search { .. } => {
                reply.search =
                    Some(super::filesystem_search::search(path, &request.operation).await?);
                Ok(Vec::new())
            }
            FileSystemAction::Mkdir { parents, exist_ok } => {
                self.mkdir(path, *parents, *exist_ok, reply).await?;
                Ok(Vec::new())
            }
            FileSystemAction::Stat { follow_symlinks } => {
                let value = fs::symlink_metadata(path)
                    .await
                    .map_err(|error| io_error("stat", error))?;
                let linked = is_link(&value);
                let link_target = if linked {
                    Some(
                        fs::read_link(path)
                            .await
                            .map_err(|error| io_error("read_link", error))?
                            .to_str()
                            .ok_or_else(|| {
                                FileError::new(
                                    "non_utf8_path",
                                    "read_link",
                                    "link target cannot be represented as UTF-8",
                                )
                            })?
                            .to_owned(),
                    )
                } else {
                    None
                };
                if !follow_symlinks && let Some(parent) = path.parent() {
                    no_links(parent, false).await?;
                }
                let value = if *follow_symlinks {
                    fs::metadata(path)
                        .await
                        .map_err(|error| io_error("stat_target", error))?
                } else {
                    value
                };
                let mut meta = metadata(&value);
                meta.is_link = linked;
                meta.link_target = link_target;
                reply.metadata = Some(meta);
                Ok(Vec::new())
            }
            FileSystemAction::Read {
                range,
                encoding,
                expected_hash,
            } => {
                if matches!(range, pab_protocol::TextReadRange::Stream { .. }) {
                    return super::filesystem_log::read(path, range, *encoding, reply).await;
                }
                let (bytes, value) = load(path).await?;
                check_hash(&bytes, expected_hash.as_deref())?;
                let doc = text::decode(&bytes, *encoding)?;
                let (data, position) = text::read(&doc, range)?;
                let mut meta = metadata(&value);
                meta.sha256 = Some(digest(&bytes));
                meta.encoding = Some(doc.encoding);
                meta.newline = Some(text::newline(&doc.text));
                reply.metadata = Some(meta);
                reply.range = Some(position);
                reply.data_size = data.len() as u32;
                reply.data_sha256 = Some(digest(&data));
                Ok(data)
            }
            FileSystemAction::Write {
                encoding,
                overwrite,
                expected_hash,
            } => {
                let content = std::str::from_utf8(payload).map_err(|_| {
                    FileError::new(
                        "invalid_encoding",
                        "input",
                        "write payload must be UTF-8 text",
                    )
                })?;
                let bytes = text::encode(content, *encoding, true)?;
                self.publish_text(
                    request,
                    &bytes,
                    *overwrite,
                    expected_hash.as_deref(),
                    Some((*encoding, text::newline(content))),
                    reply,
                )
                .await?;
                Ok(Vec::new())
            }
            FileSystemAction::Patch {
                expected_hash,
                encoding,
                dry_run,
            } => {
                let edits: Vec<TextEdit> = serde_json::from_slice(payload)
                    .map_err(|error| FileError::new("invalid_edits", "input", error.to_string()))?;
                let _guard = self
                    .upload_locks
                    .try_acquire(path)
                    .await
                    .map_err(|error| io_error("lock", error))?
                    .ok_or_else(|| {
                        FileError::new(
                            "path_busy",
                            "lock",
                            "another PAB operation is writing this path",
                        )
                    })?;
                let (original, _) = load(path).await?;
                check_hash(&original, Some(expected_hash))?;
                let doc = text::decode(&original, *encoding)?;
                let updated = text::patch(&doc.text, &edits)?;
                let bytes = text::encode(&updated, doc.encoding, doc.bom > 0)?;
                if *dry_run {
                    reply.patch_preview = Some(pab_protocol::PatchPreview {
                        original_sha256: digest(&original),
                        result_sha256: digest(&bytes),
                        result_size: bytes.len() as u64,
                        changed: original != bytes,
                        matched_edits: edits.iter().map(|e| e.expected_matches).collect(),
                    });
                    return Ok(Vec::new());
                }
                self.publish_locked(
                    request,
                    &bytes,
                    true,
                    Some(expected_hash),
                    Some((doc.encoding, text::newline(&updated))),
                    reply,
                )
                .await?;
                Ok(Vec::new())
            }
        }
    }
}

#[cfg(test)]
#[path = "filesystem_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "filesystem_b2_tests.rs"]
mod b2_tests;

#[cfg(test)]
#[path = "filesystem_b3_tests.rs"]
mod b3_tests;

#[cfg(test)]
#[path = "filesystem_enhanced_tests.rs"]
mod enhanced_tests;
