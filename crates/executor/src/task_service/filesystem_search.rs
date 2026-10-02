use super::{filesystem::FileError, filesystem_io as io, filesystem_text as text};
use globset::{GlobBuilder, GlobSetBuilder};
use pab_protocol::{
    FileSearchMatch, FileSearchMode, FileSearchSummary, FileSystemAction, SearchContextLine,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::fs;
const MAX_ENTRIES: u32 = 4096;
const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RESULT_BYTES: usize = 16 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    request: String,
    tree: String,
    index: usize,
    line: usize,
    file_hash: Option<String>,
}
struct Entry {
    path: PathBuf,
    relative: String,
    meta: std::fs::Metadata,
}

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
        options,
    } = operation
    else {
        unreachable!()
    };
    let compile = |pattern: &str| {
        GlobBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .literal_separator(true)
            .backslash_escape(true)
            .build()
            .map_err(|e| FileError::new("invalid_glob", "validate", e.to_string()))
    };
    let matcher = compile(glob)?.compile_matcher();
    let mut excludes = GlobSetBuilder::new();
    for pattern in &options.exclude {
        excludes.add(compile(pattern)?);
    }
    let excludes = excludes
        .build()
        .map_err(|e| FileError::new("invalid_glob", "validate", e.to_string()))?;
    let expression = if options.regex {
        query.clone()
    } else {
        regex::escape(query)
    };
    let regex = regex::RegexBuilder::new(&expression)
        .case_insensitive(!case_sensitive)
        .size_limit(1024 * 1024)
        .dfa_size_limit(1024 * 1024)
        .build()
        .map_err(|e| FileError::new("invalid_regex", "validate", e.to_string()))?;
    let key = io::digest(
        &serde_json::to_vec(&(
            path.to_string_lossy(),
            mode,
            query,
            glob,
            case_sensitive,
            max_depth,
            max_file_bytes,
            options.regex,
            &options.exclude,
            options.context_lines,
        ))
        .map_err(|e| FileError::new("invalid_request", "validate", e.to_string()))?,
    );
    let cursor: Option<Cursor> = options
        .cursor
        .as_ref()
        .map(|s| {
            serde_json::from_str(s)
                .map_err(|_| FileError::new("invalid_cursor", "validate", "invalid search cursor"))
        })
        .transpose()?;
    if cursor.as_ref().is_some_and(|c| {
        c.version != 1
            || c.request != key
            || c.index > MAX_ENTRIES as usize
            || c.line > 4 * 1024 * 1024
    }) {
        return Err(FileError::new(
            "invalid_cursor",
            "validate",
            "cursor does not match the search parameters",
        ));
    }
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
                entries.push(Entry {
                    path: full,
                    relative,
                    meta: value,
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
    entries.sort_by(|a, b| a.relative.cmp(&b.relative));
    let mut fingerprint = String::new();
    for e in &entries {
        fingerprint.push_str(&format!(
            "{:?}|{}|{:?}|{:?}|{}\n",
            e.relative,
            e.meta.len(),
            e.meta.modified().ok(),
            e.meta.created().ok(),
            e.meta.is_dir()
        ));
    }
    let tree = io::digest(fingerprint.as_bytes());
    if let Some(c) = &cursor
        && (c.tree != tree || c.index > entries.len() || summary.truncated)
    {
        return Err(FileError::new(
            "search_changed",
            "search",
            "directory inventory changed; start a fresh search",
        ));
    }
    let resumable = !summary.truncated;
    let begin = cursor.as_ref().map_or(0, |c| c.index);
    for (index, entry) in entries.iter().enumerate().skip(begin) {
        let mut line_start = if index == begin {
            cursor.as_ref().map_or(0, |c| c.line)
        } else {
            0
        };
        if tokio::time::Instant::now() >= deadline {
            resume(
                &mut summary,
                "time_limit",
                resumable,
                &key,
                &tree,
                index,
                line_start,
                cursor
                    .as_ref()
                    .filter(|_| index == begin)
                    .and_then(|c| c.file_hash.clone()),
            );
            break;
        }
        if !matcher.is_match(&entry.relative) {
            continue;
        }
        if *mode == FileSearchMode::Name {
            if regex.is_match(
                entry
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(""),
            ) {
                let m = FileSearchMatch {
                    path: entry.path.to_string_lossy().into_owned(),
                    kind: if entry.meta.is_dir() {
                        "directory"
                    } else {
                        "file"
                    }
                    .into(),
                    line: None,
                    preview: None,
                    sha256: None,
                    context: vec![],
                };
                if !push(&mut summary, m) {
                    if summary.matches.is_empty() {
                        return Err(FileError::new(
                            "result_too_large",
                            "search",
                            "one match exceeds the output budget; reduce context_lines or narrow the search",
                        ));
                    }
                    resume(
                        &mut summary,
                        "output_bytes_limit",
                        resumable,
                        &key,
                        &tree,
                        index,
                        0,
                        None,
                    );
                    break;
                }
                if summary.matches.len() >= *max_results as usize {
                    resume(
                        &mut summary,
                        "match_limit",
                        resumable,
                        &key,
                        &tree,
                        index + 1,
                        0,
                        None,
                    );
                    break;
                }
            }
            continue;
        }
        if !entry.meta.is_file() {
            continue;
        }
        if entry.meta.len() > *max_file_bytes as u64 {
            warn(&mut summary, &entry.path, "file_size_limit");
            continue;
        }
        if summary.scanned_bytes + *max_file_bytes as u64 + 1 > MAX_SCAN_BYTES {
            resume(
                &mut summary,
                "scan_bytes_limit",
                resumable,
                &key,
                &tree,
                index,
                line_start,
                None,
            );
            break;
        }
        summary.scanned_bytes += *max_file_bytes as u64 + 1;
        let loaded = tokio::time::timeout_at(
            deadline,
            io::load_limited(&entry.path, *max_file_bytes as usize),
        )
        .await;
        let (bytes, _) = match loaded {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                warn(&mut summary, &entry.path, &e.0.code);
                continue;
            }
            Err(_) => {
                resume(
                    &mut summary,
                    "time_limit",
                    resumable,
                    &key,
                    &tree,
                    index,
                    line_start,
                    None,
                );
                break;
            }
        };
        summary.scanned_bytes -= (*max_file_bytes as usize + 1).saturating_sub(bytes.len()) as u64;
        let hash = io::digest(&bytes);
        if index == begin
            && cursor
                .as_ref()
                .and_then(|c| c.file_hash.as_ref())
                .is_some_and(|h| *h != hash)
        {
            return Err(FileError::new(
                "search_changed",
                "search",
                "continuation file contents changed",
            ));
        }
        let doc = match text::decode(&bytes, None) {
            Ok(v) => v,
            Err(e) => {
                warn(&mut summary, &entry.path, &e.0.code);
                continue;
            }
        };
        let normalized = doc.text.replace("\r\n", "\n").replace('\r', "\n");
        let lines: Vec<_> = normalized.split('\n').collect();
        if line_start > lines.len() {
            return Err(FileError::new("invalid_cursor", "search", "line past EOF"));
        }
        while line_start < lines.len() {
            if tokio::time::Instant::now() >= deadline {
                resume(
                    &mut summary,
                    "time_limit",
                    resumable,
                    &key,
                    &tree,
                    index,
                    line_start,
                    Some(hash.clone()),
                );
                return Ok(summary);
            }
            let content = lines[line_start];
            if regex.is_match(content) {
                let context = options.context_lines as usize;
                let m = FileSearchMatch {
                    path: entry.path.to_string_lossy().into_owned(),
                    kind: "file".into(),
                    line: Some(line_start as u64 + 1),
                    preview: Some(content.chars().take(160).collect()),
                    sha256: Some(hash.clone()),
                    context: if context == 0 {
                        vec![]
                    } else {
                        (line_start.saturating_sub(context)
                            ..(line_start + context + 1).min(lines.len()))
                            .map(|i| SearchContextLine {
                                line: i as u64 + 1,
                                text: lines[i].chars().take(160).collect(),
                            })
                            .collect()
                    },
                };
                if !push(&mut summary, m) {
                    if summary.matches.is_empty() {
                        return Err(FileError::new(
                            "result_too_large",
                            "search",
                            "one match exceeds the output budget; reduce context_lines or narrow the search",
                        ));
                    }
                    resume(
                        &mut summary,
                        "output_bytes_limit",
                        resumable,
                        &key,
                        &tree,
                        index,
                        line_start,
                        Some(hash),
                    );
                    return Ok(summary);
                }
                if summary.matches.len() >= *max_results as usize {
                    resume(
                        &mut summary,
                        "match_limit",
                        resumable,
                        &key,
                        &tree,
                        index,
                        line_start + 1,
                        Some(hash),
                    );
                    return Ok(summary);
                }
            }
            line_start += 1;
        }
    }
    Ok(summary)
}
fn push(summary: &mut FileSearchSummary, value: FileSearchMatch) -> bool {
    summary.matches.push(value);
    if serde_json::to_vec(summary).map_or(true, |b| b.len() > MAX_RESULT_BYTES - 2048) {
        summary.matches.pop();
        return false;
    }
    true
}
fn resume(
    s: &mut FileSearchSummary,
    reason: &str,
    allowed: bool,
    key: &str,
    tree: &str,
    index: usize,
    line: usize,
    file_hash: Option<String>,
) {
    stop(s, reason);
    if allowed {
        s.next_cursor = serde_json::to_string(&Cursor {
            version: 1,
            request: key.into(),
            tree: tree.into(),
            index,
            line,
            file_hash,
        })
        .ok();
    }
}
fn warn(s: &mut FileSearchSummary, path: &Path, reason: &str) {
    s.skipped_entries += 1;
    if s.warnings.len() < 8 {
        s.warnings.push(
            format!("{}: {reason}", path.display())
                .chars()
                .take(160)
                .collect(),
        );
    }
}
fn stop(s: &mut FileSearchSummary, reason: &str) {
    s.truncated = true;
    if s.stop_reason.is_none() {
        s.stop_reason = Some(reason.into());
    }
}
