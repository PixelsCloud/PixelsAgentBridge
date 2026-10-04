use super::{TaskService, filesystem::FileError, filesystem_io as io};
use pab_protocol::FileSystemReply;
use std::path::Path;
use tokio::fs;

impl TaskService {
    pub(super) async fn mkdir(
        &self,
        path: &Path,
        parents: bool,
        exist_ok: bool,
        reply: &mut FileSystemReply,
    ) -> Result<(), FileError> {
        // Record every directory actually created, including partial failure. Do not roll it back.
        reply.created_paths = Some(Vec::new());
        let parts = path.ancestors().collect::<Vec<_>>();
        if parts.len() > 128 {
            return Err(FileError::new(
                "depth_limit",
                "validate",
                "mkdir supports at most 128 path components",
            ));
        }
        let mut missing = Vec::new();
        for part in &parts {
            match fs::symlink_metadata(part).await {
                Ok(value) => {
                    #[cfg(target_os = "macos")]
                    let value = io::macos_system_alias(part, &value).unwrap_or(value);
                    io::no_links(part, false).await?;
                    if !value.is_dir() {
                        return Err(FileError::new(
                            "not_directory",
                            "mkdir",
                            "an existing path component is not a directory",
                        ));
                    }
                    if missing.is_empty() {
                        if !exist_ok {
                            return Err(FileError::new(
                                "already_exists",
                                "mkdir",
                                "directory already exists; set exist_ok=true explicitly",
                            ));
                        }
                        reply.metadata = Some(io::metadata(&value));
                        return Ok(());
                    }
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(part.to_path_buf())
                }
                Err(error) => return Err(io::io_error("inspect_path", error)),
            }
        }
        if !parents && missing.len() > 1 {
            return Err(FileError::new(
                "missing_parent",
                "mkdir",
                "parent directory is missing; set parents=true explicitly",
            ));
        }
        let report = missing
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        if serde_json::to_vec(&report).map_or(true, |bytes| bytes.len() > 12 * 1024) {
            return Err(FileError::new(
                "output_bytes_limit",
                "validate",
                "created-path reporting would exceed its budget",
            ));
        }
        for target in missing.into_iter().rev() {
            io::no_links(
                target
                    .parent()
                    .ok_or_else(|| FileError::new("invalid_path", "mkdir", "path has no parent"))?,
                false,
            )
            .await?;
            let _guard = self
                .upload_locks
                .try_acquire(&target)
                .await
                .map_err(|e| io::io_error("lock", e))?
                .ok_or_else(|| {
                    FileError::new(
                        "path_busy",
                        "lock",
                        "another PAB operation is using this path",
                    )
                })?;
            match fs::create_dir(&target).await {
                Ok(()) => reply
                    .created_paths
                    .as_mut()
                    .unwrap()
                    .push(target.to_string_lossy().into_owned()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    io::no_links(&target, false).await?;
                    if !fs::metadata(&target)
                        .await
                        .map_err(|e| io::io_error("stat", e))?
                        .is_dir()
                        || (target == path && !exist_ok)
                    {
                        return Err(io::io_error("mkdir", error));
                    }
                }
                Err(error) => return Err(io::io_error("mkdir", error)),
            }
            self.store
                .update_filesystem_progress(reply)
                .await
                .map_err(|e| FileError::new("record_error", "mkdir", e.to_string()))?;
        }
        io::no_links(path, false).await?;
        reply.metadata = Some(io::metadata(
            &fs::metadata(path)
                .await
                .map_err(|e| io::io_error("stat", e))?,
        ));
        Ok(())
    }
}
