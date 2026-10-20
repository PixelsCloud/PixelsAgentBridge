use super::{filesystem::FileError, filesystem_io as io};
use pab_file_codec::search::{self, Entry, MAX_ENTRIES, SearchSource, stop};
use pab_protocol::{FileSearchSummary, FileSystemAction};
use std::{path::Path, time::Duration};
use tokio::fs;
struct DesktopSource;
impl SearchSource for DesktopSource {
    async fn load(&self, path: &str, limit: usize) -> Result<Vec<u8>, FileError> {
        Ok(io::load_limited(Path::new(path), limit).await?.0)
    }
}
fn warn(s: &mut FileSearchSummary, path: &Path, reason: &str) {
    search::warn(s, &path.to_string_lossy(), reason);
}
pub(super) async fn search(
    path: &Path,
    operation: &FileSystemAction,
) -> Result<FileSearchSummary, FileError> {
    let FileSystemAction::Search { max_depth, .. } = operation else {
        unreachable!()
    };
    let patterns = search::Patterns::new(operation)?;
    let excludes = &patterns.excludes;
    io::no_links(path, false).await?;
    if !fs::metadata(path)
        .await
        .map_err(|e| io::io_error("stat", e))?
        .is_dir()
    {
        return Err(FileError::new(
            "not_directory",
            "search",
            "search root must be a directory",
        ));
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut summary = FileSearchSummary::default();
    let mut entries = Vec::new();
    let mut pending = vec![(path.to_path_buf(), 0u32)];
    // Build a bounded, sorted metadata inventory. A continuation refuses observed tree changes.
    let inventory = async {
        while let Some((dir, depth)) = pending.pop() {
            io::no_links(&dir, false).await?;
            let mut iter = match fs::read_dir(&dir).await {
                Ok(v) => v,
                Err(e) => {
                    warn(&mut summary, &dir, &e.to_string());
                    continue;
                }
            };
            while let Some(entry) = iter
                .next_entry()
                .await
                .map_err(|e| io::io_error("read_dir", e))?
            {
                if summary.scanned_entries >= MAX_ENTRIES {
                    stop(&mut summary, "entry_limit");
                    return Ok::<_, FileError>(());
                }
                summary.scanned_entries += 1;
                let full = entry.path();
                let Some(relative) = full.strip_prefix(path).ok().and_then(Path::to_str) else {
                    warn(&mut summary, &full, "non_utf8_path");
                    continue;
                };
                let relative = relative.replace('\\', "/");
                let value = match fs::symlink_metadata(&full).await {
                    Ok(v) => v,
                    Err(e) => {
                        warn(&mut summary, &full, &e.to_string());
                        continue;
                    }
                };
                if excludes.is_match(&relative)
                    || (value.is_dir() && excludes.is_match(format!("{relative}/")))
                {
                    continue;
                }
                if io::is_link(&value) {
                    warn(&mut summary, &full, "link_not_followed");
                    continue;
                }
                if value.is_dir() {
                    if depth + 1 < *max_depth {
                        pending.push((full.clone(), depth + 1));
                    } else {
                        warn(&mut summary, &full, "depth_limit");
                        stop(&mut summary, "depth_limit");
                    }
                }
                let fingerprint = format!(
                    "{:?}|{}|{:?}|{:?}|{}\n",
                    relative,
                    value.len(),
                    value.modified().ok(),
                    value.created().ok(),
                    value.is_dir()
                );
                entries.push(Entry {
                    path: full.to_string_lossy().into_owned(),
                    name: entry.file_name().to_string_lossy().into_owned(),
                    relative,
                    size: value.len(),
                    directory: value.is_dir(),
                    regular: value.is_file(),
                    fingerprint,
                });
            }
        }
        Ok(())
    };
    if let Ok(result) = tokio::time::timeout_at(deadline, inventory).await {
        result?;
    } else {
        stop(&mut summary, "time_limit");
    }
    search::search(
        &DesktopSource,
        &path.to_string_lossy(),
        operation,
        summary,
        entries,
        deadline,
        &patterns,
    )
    .await
}
