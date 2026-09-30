use super::{filesystem::FileError, filesystem_io as io, filesystem_text as text};
use globset::GlobBuilder;
use pab_protocol::{FileSearchMatch, FileSearchMode, FileSearchSummary, FileSystemAction};
use std::path::Path;
use tokio::fs;

const MAX_ENTRIES: u32 = 4096;
const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RESULT_BYTES: usize = 16 * 1024;

pub(super) async fn search(
    path: &Path,
    operation: &FileSystemAction,
) -> Result<FileSearchSummary, FileError> {
    let FileSystemAction::Search {
        mode,
        query,
        glob,
        case_sensitive,
        max_results,
        max_depth,
        max_file_bytes,
    } = operation
    else {
        unreachable!()
    };
    let matcher = GlobBuilder::new(glob)
        .case_insensitive(!case_sensitive)
        .literal_separator(true)
        .backslash_escape(true)
        .build()
        .map_err(|error| FileError::new("invalid_glob", "validate", error.to_string()))?
        .compile_matcher();
    io::no_links(path, false).await?;
    if !fs::metadata(path)
        .await
        .map_err(|error| io::io_error("stat", error))?
        .is_dir()
    {
        return Err(FileError::new(
            "not_directory",
            "search",
            "search root must be a directory",
        ));
    }
    let needle = if *case_sensitive {
        query.clone()
    } else {
        query.to_lowercase()
    };
    let mut summary = FileSearchSummary::default();
    let scan = async {
        let mut pending = vec![(path.to_path_buf(), 0u32)];
        while let Some((directory, depth)) = pending.pop() {
            if let Err(error) = io::no_links(&directory, false).await {
                warn(&mut summary, &directory, &error.0.code);
                continue;
            }
            let mut entries = match fs::read_dir(&directory).await {
                Ok(entries) => entries,
                Err(error) => {
                    warn(&mut summary, &directory, &error.to_string());
                    continue;
                }
            };
            loop {
                if summary.scanned_entries >= MAX_ENTRIES {
                    stop(&mut summary, "entry_limit");
                    return;
                }
                let entry = match entries.next_entry().await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => break,
                    Err(error) => {
                        warn(&mut summary, &directory, &error.to_string());
                        break;
                    }
                };
                summary.scanned_entries += 1;
                let full = entry.path();
                let Some(relative) = full.strip_prefix(path).ok().and_then(Path::to_str) else {
                    warn(&mut summary, &full, "non_utf8_path");
                    continue;
                };
                let relative = relative.replace('\\', "/");
                let value = match fs::symlink_metadata(&full).await {
                    Ok(value) => value,
                    Err(error) => {
                        warn(&mut summary, &full, &error.to_string());
                        continue;
                    }
                };
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
                if !matcher.is_match(&relative) {
                    continue;
                }
                if *mode == FileSearchMode::Name {
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else {
                        warn(&mut summary, &full, "non_utf8_path");
                        continue;
                    };
                    let name = if *case_sensitive {
                        name.to_owned()
                    } else {
                        name.to_lowercase()
                    };
                    if name.contains(&needle)
                        && !push(
                            &mut summary,
                            FileSearchMatch {
                                path: full.to_string_lossy().into_owned(),
                                kind: if value.is_dir() {
                                    "directory"
                                } else if value.is_file() {
                                    "file"
                                } else {
                                    "other"
                                }
                                .to_owned(),
                                line: None,
                                preview: None,
                                sha256: None,
                            },
                            *max_results,
                        )
                    {
                        return;
                    }
                } else if value.is_file() {
                    if value.len() > *max_file_bytes as u64 {
                        warn(&mut summary, &full, "file_size_limit");
                        continue;
                    }
                    if summary.scanned_bytes + *max_file_bytes as u64 + 1 > MAX_SCAN_BYTES {
                        stop(&mut summary, "scan_bytes_limit");
                        return;
                    }
                    // Charge the upper bound before reading, including skipped invalid text.
                    summary.scanned_bytes += *max_file_bytes as u64 + 1;
                    let (bytes, _) = match io::load_limited(&full, *max_file_bytes as usize).await {
                        Ok(pair) => pair,
                        Err(error) => {
                            warn(&mut summary, &full, &error.0.code);
                            continue;
                        }
                    };
                    summary.scanned_bytes -=
                        (*max_file_bytes as usize + 1).saturating_sub(bytes.len()) as u64;
                    if bytes.len() > *max_file_bytes as usize {
                        warn(&mut summary, &full, "file_size_changed");
                        continue;
                    }
                    let doc = match text::decode(&bytes, None) {
                        Ok(doc) => doc,
                        Err(error) => {
                            warn(&mut summary, &full, &error.0.code);
                            continue;
                        }
                    };
                    let hash = io::digest(&bytes);
                    // Treat CRLF as one line break, and support bare CR/LF.
                    let normalized = doc.text.replace("\r\n", "\n").replace('\r', "\n");
                    for (line, content) in normalized.split('\n').enumerate() {
                        let candidate = if *case_sensitive {
                            content.to_owned()
                        } else {
                            content.to_lowercase()
                        };
                        if candidate.contains(&needle)
                            && !push(
                                &mut summary,
                                FileSearchMatch {
                                    path: full.to_string_lossy().into_owned(),
                                    kind: "file".to_owned(),
                                    line: Some(line as u64 + 1),
                                    preview: Some(content.chars().take(160).collect()),
                                    sha256: Some(hash.clone()),
                                },
                                *max_results,
                            )
                        {
                            return;
                        }
                    }
                }
            }
        }
    };
    if tokio::time::timeout(std::time::Duration::from_secs(5), scan)
        .await
        .is_err()
    {
        stop(&mut summary, "time_limit");
    }
    Ok(summary)
}

fn push(summary: &mut FileSearchSummary, value: FileSearchMatch, limit: u32) -> bool {
    summary.matches.push(value);
    if serde_json::to_vec(summary).map_or(true, |bytes| bytes.len() > MAX_RESULT_BYTES) {
        summary.matches.pop();
        stop(summary, "output_bytes_limit");
        return false;
    }
    if summary.matches.len() >= limit as usize {
        stop(summary, "match_limit");
        return false;
    }
    true
}

fn warn(summary: &mut FileSearchSummary, path: &Path, reason: &str) {
    summary.skipped_entries += 1;
    if summary.warnings.len() < 8 {
        summary.warnings.push(
            format!("{}: {reason}", path.display())
                .chars()
                .take(160)
                .collect(),
        );
    }
}

fn stop(summary: &mut FileSearchSummary, reason: &str) {
    summary.truncated = true;
    if summary.stop_reason.is_none() {
        summary.stop_reason = Some(reason.to_owned());
    }
}
