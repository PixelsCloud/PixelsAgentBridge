//! Android inventory and snapshots feed the same search engine as desktop.
use super::*;
use pab_file_codec::search::{self, Entry, MAX_ENTRIES, Patterns, SearchSource};
struct Source<'a> {
    service: &'a FileService,
    cancel: &'a AtomicBool,
}
impl SearchSource for Source<'_> {
    async fn load(&self, path: &str, limit: usize) -> Result<Vec<u8>, FileError> {
        cancelled(self.cancel)?;
        let (stage, _) = self.service.snapshot(path, limit as u64).await?;
        cancelled(self.cancel)?;
        tokio::fs::read(&stage.0).await.map_err(io_error)
    }
    fn check(&self) -> Result<(), FileError> {
        cancelled(self.cancel)
    }
}
impl FileService {
    pub(super) async fn search(
        &self,
        owner: &str,
        request: &FileSystemRequest,
        cancel: &AtomicBool,
    ) -> Result<FileSearchSummary, FileError> {
        let mut operation = request.operation.clone();
        if let FileSystemAction::Search { options, .. } = &mut operation
            && let Some(token) = options.cursor.take()
        {
            options.cursor = Some(self.open_cursor(owner, &request.path, "search", &token)?);
        }
        let patterns = Patterns::new(&operation)?;
        let FileSystemAction::Search { max_depth, .. } = &operation else {
            unreachable!()
        };
        let root = self
            .io(json!({"action":"stat","path":request.path}))
            .await?;
        if root["metadata"]["directory"] != true {
            return Err(FileError::new(
                "not_directory",
                "search",
                "Search root must be a directory",
            ));
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut summary = FileSearchSummary::default();
        let mut entries = Vec::new();
        let mut pending = vec![(
            request.path.trim_end_matches('/').to_owned(),
            String::new(),
            0u32,
        )];
        let inventory = async {
            while let Some((directory, relative, depth)) = pending.pop() {
                let mut after = None;
                loop {
                    cancelled(cancel)?;
                    let page = match self
                        .io(json!({"action":"list","path":directory,"after":after,"limit":64}))
                        .await
                    {
                        Ok(value) => value,
                        Err(error) => {
                            search::warn(&mut summary, &directory, &error.0.code);
                            search::stop(&mut summary, "inventory_incomplete");
                            break;
                        }
                    };
                    let rows = page["entries"].as_array().ok_or_else(|| {
                        FileError::new("invalid_result", "search", "Missing directory entries")
                    })?;
                    for entry in rows {
                        cancelled(cancel)?;
                        if summary.scanned_entries >= MAX_ENTRIES {
                            search::stop(&mut summary, "entry_limit");
                            return Ok::<_, FileError>(());
                        }
                        summary.scanned_entries += 1;
                        let entry_name = entry["name"].as_str().ok_or_else(|| {
                            FileError::new("invalid_result", "search", "Missing entry name")
                        })?;
                        if entry_name.is_empty()
                            || entry_name.len() > 255
                            || entry_name.contains(['/', '\\', '\0'])
                            || matches!(entry_name, "." | "..")
                        {
                            return Err(FileError::new(
                                "invalid_result",
                                "search",
                                "Invalid entry name",
                            ));
                        }
                        let path = format!("{directory}/{entry_name}");
                        let name = if relative.is_empty() {
                            entry_name.to_owned()
                        } else {
                            format!("{relative}/{entry_name}")
                        };
                        let is_directory = entry["directory"] == true;
                        if patterns.excludes.is_match(&name)
                            || (is_directory && patterns.excludes.is_match(format!("{name}/")))
                        {
                            continue;
                        }
                        if is_directory {
                            if depth + 1 < *max_depth {
                                pending.push((path.clone(), name.clone(), depth + 1));
                            } else {
                                search::warn(&mut summary, &path, "depth_limit");
                                search::stop(&mut summary, "depth_limit");
                            }
                        }
                        let fingerprint = json!([name, entry]).to_string();
                        entries.push(Entry {
                            path,
                            name: entry_name.into(),
                            relative: name,
                            size: entry["size"].as_u64().unwrap_or(0),
                            directory: is_directory,
                            regular: !is_directory,
                            fingerprint,
                        });
                    }
                    if page["has_more"] != true {
                        break;
                    }
                    after = rows
                        .last()
                        .and_then(|v| v["name"].as_str())
                        .map(str::to_owned);
                    if after.is_none() {
                        return Err(FileError::new(
                            "invalid_result",
                            "search",
                            "Empty continuation page",
                        ));
                    }
                }
            }
            Ok(())
        };
        match tokio::time::timeout_at(deadline, inventory).await {
            Ok(result) => result?,
            Err(_) => search::stop(&mut summary, "time_limit"),
        }
        cancelled(cancel)?;
        let mut result = search::search(
            &Source {
                service: self,
                cancel,
            },
            &format!("{owner}:{}", request.path),
            &operation,
            summary,
            entries,
            deadline,
            &patterns,
        )
        .await?;
        if let Some(token) = result.next_cursor.take() {
            result.next_cursor = Some(self.seal_cursor(owner, &request.path, "search", &token)?);
        }
        Ok(result)
    }
}
