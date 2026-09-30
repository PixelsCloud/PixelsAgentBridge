use super::{
    TaskService,
    filesystem::FileError,
    filesystem_io::{Staging, check_hash, digest, io_error, load, metadata, no_links},
};
use pab_protocol::{FileSystemReply, FileSystemRequest, RequestId};
use std::path::Path;
use tokio::{fs, io::AsyncWriteExt};

impl TaskService {
    pub(super) async fn publish_text(
        &self,
        request: &FileSystemRequest,
        bytes: &[u8],
        overwrite: bool,
        expected: Option<&str>,
        text_meta: Option<(pab_protocol::TextEncoding, String)>,
        reply: &mut FileSystemReply,
    ) -> Result<(), FileError> {
        let path = Path::new(&request.path);
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
        self.publish_locked(request, bytes, overwrite, expected, text_meta, reply)
            .await
    }

    pub(super) async fn publish_locked(
        &self,
        request: &FileSystemRequest,
        bytes: &[u8],
        overwrite: bool,
        expected: Option<&str>,
        text_meta: Option<(pab_protocol::TextEncoding, String)>,
        reply: &mut FileSystemReply,
    ) -> Result<(), FileError> {
        let path = Path::new(&request.path);
        no_links(path, true).await?;
        let original = match fs::symlink_metadata(path).await {
            Ok(value) => {
                if !overwrite {
                    return Err(FileError::new(
                        "already_exists",
                        "validate",
                        "destination exists; overwrite must be explicit",
                    ));
                }
                if !value.is_file() {
                    return Err(FileError::new(
                        "not_regular_file",
                        "validate",
                        "destination is not an ordinary file",
                    ));
                }
                if value.permissions().readonly() {
                    return Err(FileError::new(
                        "access_denied",
                        "validate",
                        "destination is readonly",
                    ));
                }
                if expected.is_some() {
                    let (data, _) = load(path).await?;
                    check_hash(&data, expected)?;
                }
                Some(value)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if expected.is_some() {
                    return Err(FileError::new(
                        "version_conflict",
                        "validate",
                        "expected existing file is missing",
                    ));
                }
                None
            }
            Err(error) => return Err(io_error("stat", error)),
        };
        let staging_path = path
            .parent()
            .ok_or_else(|| FileError::new("invalid_path", "validate", "missing parent"))?
            .join(format!(
                ".pab-text-{}-{}.tmp",
                request.request_id,
                RequestId::new()
            ));
        let staging;
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&staging_path)
            .await
            .map_err(|error| io_error("create_staging", error))?;
        staging = Staging(staging_path);
        file.write_all(bytes)
            .await
            .map_err(|error| io_error("write_staging", error))?;
        if let Some(value) = &original {
            file.set_permissions(value.permissions())
                .await
                .map_err(|error| io_error("preserve_permissions", error))?;
        }
        file.sync_all()
            .await
            .map_err(|error| io_error("sync_staging", error))?;
        let mut meta = metadata(
            &file
                .metadata()
                .await
                .map_err(|error| io_error("stat_staging", error))?,
        );
        drop(file);
        no_links(path, true).await?;
        if let Some(value) = &original {
            let current = fs::symlink_metadata(path)
                .await
                .map_err(|error| io_error("validate_version", error))?;
            if !current.is_file()
                || value.len() != current.len()
                || value.modified().ok() != current.modified().ok()
            {
                return Err(FileError::new(
                    "version_conflict",
                    "validate_version",
                    "destination changed before publication",
                ));
            }
        } else if fs::symlink_metadata(path).await.is_ok() {
            return Err(FileError::new(
                "version_conflict",
                "validate_version",
                "destination appeared before publication",
            ));
        }
        if expected.is_some() {
            let (data, _) = load(path).await?;
            check_hash(&data, expected)?;
        }
        meta.sha256 = Some(digest(bytes));
        if let Some((encoding, newline)) = text_meta {
            meta.encoding = Some(encoding);
            meta.newline = Some(newline);
        }
        reply.metadata = Some(meta);
        self.store
            .begin_file_publication(reply)
            .await
            .map_err(|error| {
                FileError::new(
                    "storage_unavailable",
                    "record_publication",
                    error.to_string(),
                )
            })?;
        if overwrite && original.is_some() {
            fs::rename(&staging.0, path)
                .await
                .map_err(|error| io_error("publish", error))?;
        } else {
            fs::hard_link(&staging.0, path)
                .await
                .map_err(|error| io_error("publish", error))?;
        }
        Ok(())
    }
}
