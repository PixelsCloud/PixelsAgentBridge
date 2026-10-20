//! Plan bounded virtual trees before changing storage; report partial mutations explicitly.
use super::*;

#[derive(Clone)]
pub(super) struct PlannedEntry {
    pub path: String,
    pub relative: String,
    pub value: Value,
}

impl FileService {
    pub(super) async fn inventory(
        &self,
        root: &str,
        recursive: bool,
        limits: &FileOperationLimits,
        cancel: &AtomicBool,
    ) -> Result<Vec<PlannedEntry>, FileError> {
        let mut pending = vec![(root.trim_end_matches('/').to_owned(), String::new(), 0u32)];
        let mut entries = Vec::new();
        let mut total = 0u64;
        while let Some((path, relative, depth)) = pending.pop() {
            cancelled(cancel)?;
            if entries.len() >= limits.max_entries as usize {
                return Err(limit("entry_limit"));
            }
            let value = self.io(json!({"action":"stat","path":path})).await?["metadata"].clone();
            let directory = value["directory"] == true;
            if directory {
                let mut after = None;
                loop {
                    cancelled(cancel)?;
                    let page = self.directory(RequestId::new(), &path, after, 64).await?;
                    if !recursive && !page.entries.is_empty() {
                        return Err(FileError::new(
                            "recursive_required",
                            "planning",
                            "A nonempty directory requires recursive=true",
                        ));
                    }
                    for child in page.entries {
                        if depth + 1 >= limits.max_depth {
                            return Err(limit("depth_limit"));
                        }
                        if pending.len() + entries.len() + 1 >= limits.max_entries as usize {
                            return Err(limit("entry_limit"));
                        }
                        let relative = if relative.is_empty() {
                            child.name.clone()
                        } else {
                            format!("{relative}/{}", child.name)
                        };
                        pending.push((format!("{path}/{}", child.name), relative, depth + 1));
                    }
                    after = page.next_after;
                    if after.is_none() {
                        break;
                    }
                }
            } else {
                let size = value["size"].as_u64().ok_or_else(|| {
                    FileError::new(
                        "size_unavailable",
                        "planning",
                        "Provider must report file size for bounded bulk operations",
                    )
                })?;
                if size > 1024 * 1024 * 1024 {
                    return Err(limit("file_size_limit"));
                }
                total = total
                    .checked_add(size)
                    .ok_or_else(|| limit("bytes_limit"))?;
                if total > limits.max_bytes {
                    return Err(limit("bytes_limit"));
                }
            }
            entries.push(PlannedEntry {
                path,
                relative,
                value,
            });
        }
        entries.sort_by(|a, b| {
            a.relative
                .split('/')
                .count()
                .cmp(&b.relative.split('/').count())
                .then(a.relative.cmp(&b.relative))
        });
        Ok(entries)
    }

    pub(super) async fn inspect_destination(
        &self,
        path: &str,
        directory: bool,
        overwrite: bool,
    ) -> Result<Option<Value>, FileError> {
        match self.io(json!({"action":"stat","path":path})).await {
            Ok(value) => {
                let mut meta = value["metadata"].clone();
                if meta["directory"] != directory {
                    return Err(FileError::new(
                        "type_conflict",
                        "planning",
                        "Destination type conflicts with the source",
                    ));
                }
                if !directory && !overwrite {
                    return Err(FileError::new(
                        "already_exists",
                        "planning",
                        "Destination already exists",
                    ));
                }
                if meta["readonly"] == true {
                    return Err(FileError::new(
                        "access_denied",
                        "planning",
                        "Destination is read-only",
                    ));
                }
                if !directory {
                    let size = meta["size"]
                        .as_u64()
                        .ok_or_else(|| limit("size_unavailable"))?;
                    let (_, snapshot) = self.snapshot(path, size).await?;
                    if !same_version(&meta, &snapshot["metadata"]) {
                        return Err(limit("version_conflict"));
                    }
                    meta["sha256"] = snapshot["sha256"].clone();
                }
                Ok(Some(meta))
            }
            Err(error) if error.0.code == "not_found" => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(super) async fn bulk(
        &self,
        owner: &str,
        request: &FileSystemRequest,
        reply: &mut FileSystemReply,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Vec<u8>, FileError> {
        reply.mutation = Some(FileMutationSummary {
            phase: "planning".into(),
            ..Default::default()
        });
        let outcome = self.bulk_inner(owner, request, reply, cancel).await;
        let summary = reply.mutation.as_mut().unwrap();
        summary.partial =
            outcome.is_err() && (summary.published_entries > 0 || summary.deleted_entries > 0);
        if outcome.is_ok() {
            summary.phase = "completed".into();
        }
        outcome.map(|()| Vec::new())
    }

    async fn bulk_inner(
        &self,
        owner: &str,
        request: &FileSystemRequest,
        reply: &mut FileSystemReply,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(), FileError> {
        let (destination, recursive, overwrite, limits, moving, deleting) = match &request.operation
        {
            FileSystemAction::Copy {
                destination,
                recursive,
                overwrite,
                limits,
            } => (
                Some(destination),
                *recursive,
                *overwrite,
                limits,
                false,
                false,
            ),
            FileSystemAction::Move {
                destination,
                recursive,
                overwrite,
                limits,
            } => (
                Some(destination),
                *recursive,
                *overwrite,
                limits,
                true,
                false,
            ),
            FileSystemAction::Delete { recursive, limits } => {
                (None, *recursive, false, limits, false, true)
            }
            FileSystemAction::ArchiveCreate { .. } => {
                return self.archive_create(owner, request, reply, cancel).await;
            }
            FileSystemAction::ArchiveExtract { .. } => {
                return self.archive_extract(owner, request, reply, cancel).await;
            }
            _ => unreachable!(),
        };
        let source = normalized(&request.path)?;
        if moving || deleting {
            protect_root(&source)?;
        }
        if let Some(destination) = destination {
            let destination = normalized(destination)?;
            protect_root(&destination)?;
            separate(&source, &destination)?;
        }
        let entries = self.inventory(&source, recursive, limits, cancel).await?;
        reply.mutation.as_mut().unwrap().total_entries = entries.len() as u32;
        reply.progress.as_mut().unwrap().total_bytes = entries
            .iter()
            .map(|entry| entry.value["size"].as_u64().unwrap_or(0))
            .sum();
        self.bulk_progress(owner, reply).await?;
        let mut destinations = Vec::new();
        if let Some(destination) = destination {
            for entry in &entries {
                cancelled(cancel)?;
                let target = join(destination, &entry.relative);
                let previous = self
                    .inspect_destination(&target, entry.value["directory"] == true, overwrite)
                    .await?;
                destinations.push((target, previous));
            }
            let mut copied = Vec::new();
            for (entry, (target, previous)) in entries.iter().zip(&destinations) {
                cancelled(cancel)?;
                self.unchanged(entry).await?;
                reply.mutation.as_mut().unwrap().phase = "copying".into();
                if entry.value["directory"] == true {
                    if previous.is_none() {
                        self.io(json!({"action":"mkdir","path":target,"parents":false,"exist_ok":false})).await?;
                        item(reply, target, "mkdir", None, true, false);
                    }
                    copied.push(None);
                } else {
                    let (stage, value) = self
                        .snapshot(&entry.path, entry.value["size"].as_u64().unwrap())
                        .await?;
                    self.unchanged(entry).await?;
                    cancelled(cancel)?;
                    let hash = value["sha256"]
                        .as_str()
                        .ok_or_else(|| limit("invalid_snapshot"))?
                        .to_owned();
                    // Detect destination edits since planning before replacing a file.
                    if let Some(previous) = previous {
                        self.unchanged(&PlannedEntry {
                            path: target.clone(),
                            relative: String::new(),
                            value: previous.clone(),
                        })
                        .await?;
                    }
                    self.publish_stage(
                        target,
                        &stage,
                        overwrite,
                        previous.as_ref().and_then(|value| value["sha256"].as_str()),
                    )
                    .await?;
                    item(reply, target, "copy", Some(hash.clone()), true, false);
                    copied.push(Some(hash));
                    reply.progress.as_mut().unwrap().completed_bytes +=
                        entry.value["size"].as_u64().unwrap();
                }
                reply.mutation.as_mut().unwrap().processed_entries += 1;
                self.bulk_progress(owner, reply).await?;
            }
            if moving {
                // Verify all source and destination files before the first source deletion.
                for ((entry, (target, _)), hash) in entries.iter().zip(&destinations).zip(&copied) {
                    self.unchanged(entry).await?;
                    if let Some(hash) = hash {
                        cancelled(cancel)?;
                        let (_, value) = self
                            .snapshot(target, entry.value["size"].as_u64().unwrap())
                            .await?;
                        check_hash(Some(hash), value["sha256"].as_str().unwrap_or(""))?;
                    }
                }
                reply.mutation.as_mut().unwrap().phase = "deleting_source".into();
                for (entry, hash) in entries.iter().zip(&copied).rev() {
                    cancelled(cancel)?;
                    self.io(json!({"action":"delete","path":entry.path,"expected_metadata":entry.value,"expected_hash":hash})).await?;
                    item(reply, &entry.path, "delete", None, false, true);
                    self.bulk_progress(owner, reply).await?;
                }
                reply.mutation.as_mut().unwrap().source_removed = true;
            }
        } else {
            reply.mutation.as_mut().unwrap().phase = "deleting".into();
            // Complete preflight above avoids exceeding an entry/byte budget midway.
            for entry in entries.iter().rev() {
                cancelled(cancel)?;
                self.io(
                    json!({"action":"delete","path":entry.path,"expected_metadata":entry.value}),
                )
                .await?;
                item(reply, &entry.path, "delete", None, false, true);
                reply.mutation.as_mut().unwrap().processed_entries += 1;
                reply.progress.as_mut().unwrap().completed_bytes +=
                    entry.value["size"].as_u64().unwrap_or(0);
                self.bulk_progress(owner, reply).await?;
            }
        }
        Ok(())
    }

    pub(super) async fn bulk_progress(
        &self,
        owner: &str,
        reply: &FileSystemReply,
    ) -> Result<(), FileError> {
        let mut progress = reply.progress.clone();
        if let Some(progress) = &mut progress {
            progress.updated_at_unix_ms = crate::peer::now_ms();
        }
        sqlx::query("UPDATE file_operations SET reply=json_set(reply,'$.mutation',json(?),'$.progress',json(?)) WHERE owner=? AND id=? AND json_extract(reply,'$.state') IN ('running','cancel_requested')")
            .bind(serde_json::to_string(&reply.mutation).unwrap()).bind(serde_json::to_string(&progress).unwrap()).bind(owner).bind(reply.request_id.to_string())
            .execute(&self.pool).await.map_err(|_| FileError::new("progress_unconfirmed", "record", "Mutation progress could not be stored; query the original operation"))?;
        Ok(())
    }

    pub(super) async fn unchanged(&self, entry: &PlannedEntry) -> Result<(), FileError> {
        let current =
            self.io(json!({"action":"stat","path":entry.path})).await?["metadata"].clone();
        if same_version(&entry.value, &current) {
            Ok(())
        } else {
            Err(FileError::new(
                "version_conflict",
                "execute",
                "Observed file identity, size or modification time changed",
            ))
        }
    }

    pub(super) async fn publish_stage(
        &self,
        target: &str,
        stage: &StagedFile,
        overwrite: bool,
        expected: Option<&str>,
    ) -> Result<FileMetadata, FileError> {
        let id = stage
            .0
            .file_stem()
            .and_then(|part| part.to_str())
            .ok_or_else(|| limit("invalid_staging_id"))?;
        let value = self.io(json!({"action":"publish","path":target,"staging_id":id,"overwrite":overwrite,"expected_hash":expected})).await?;
        Ok(metadata(&value["metadata"]))
    }
}

pub(super) fn item(
    reply: &mut FileSystemReply,
    path: &str,
    action: &str,
    hash: Option<String>,
    published: bool,
    deleted: bool,
) {
    let summary = reply.mutation.as_mut().unwrap();
    summary.published_entries += u32::from(published);
    summary.deleted_entries += u32::from(deleted);
    if summary.results.len() < 64
        && summary
            .results
            .iter()
            .map(|entry| entry.path.len())
            .sum::<usize>()
            + path.len()
            <= 8192
    {
        summary.results.push(FileItemResult {
            path: path.into(),
            action: action.into(),
            sha256: hash,
        });
    } else {
        summary.results_truncated = true;
    }
}
pub(super) fn join(root: &str, relative: &str) -> String {
    if relative.is_empty() {
        root.into()
    } else {
        format!("{}/{relative}", root.trim_end_matches('/'))
    }
}
pub(super) fn normalized(path: &str) -> Result<String, FileError> {
    if !path.starts_with('/')
        || path.len() > 4096
        || path.contains(['\\', '\0'])
        || path[1..]
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.len() > 255)
    {
        return Err(limit("invalid_path"));
    }
    Ok(path.into())
}
pub(super) fn protect_root(path: &str) -> Result<(), FileError> {
    if path.matches('/').count() < 2 {
        Err(limit("mount_root_protected"))
    } else {
        Ok(())
    }
}
pub(super) fn separate(source: &str, target: &str) -> Result<(), FileError> {
    if source == target
        || source.starts_with(&format!("{target}/"))
        || target.starts_with(&format!("{source}/"))
    {
        Err(limit("overlapping_paths"))
    } else {
        Ok(())
    }
}
pub(super) fn same_version(before: &Value, after: &Value) -> bool {
    ["directory", "size", "modified_at_unix_ms", "identity"]
        .iter()
        .all(|key| before[*key] == after[*key])
}
pub(super) fn limit(code: &str) -> FileError {
    FileError::new(code, "planning", code)
}
