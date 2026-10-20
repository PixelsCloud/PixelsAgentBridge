//! Shared bounded search, matching, pagination and content projection.
use crate::{FileError, decode};
use globset::{GlobBuilder, GlobSetBuilder};
use pab_protocol::{
    FileSearchMatch, FileSearchMode, FileSearchSummary, FileSystemAction, SearchContextLine,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::future::Future;
use tokio::time::Instant;
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub const MAX_ENTRIES: u32 = 4096;
const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RESULT_BYTES: usize = 16 * 1024;
pub struct Entry {
    pub path: String,
    pub name: String,
    pub relative: String,
    pub size: u64,
    pub directory: bool,
    pub regular: bool,
    pub fingerprint: String,
}
pub trait SearchSource: Sync {
    fn load(
        &self,
        path: &str,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<u8>, FileError>> + Send;
    fn check(&self) -> Result<(), FileError> {
        Ok(())
    }
}
pub struct Patterns {
    pub excludes: globset::GlobSet,
    matcher: globset::GlobMatcher,
    regex: regex::Regex,
}
impl Patterns {
    pub fn new(operation: &FileSystemAction) -> Result<Self, FileError> {
        let FileSystemAction::Search {
            glob,
            query,
            case_sensitive,
            options,
            ..
        } = operation
        else {
            return Err(FileError::new(
                "invalid_request",
                "validate",
                "not a search",
            ));
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
        Ok(Self {
            excludes,
            matcher,
            regex,
        })
    }
}
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
pub async fn search<S: SearchSource>(
    source: &S,
    path_key: &str,
    operation: &FileSystemAction,
    mut summary: FileSearchSummary,
    mut entries: Vec<Entry>,
    deadline: Instant,
    patterns: &Patterns,
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
    let Patterns { matcher, regex, .. } = patterns;
    let key = digest(
        &serde_json::to_vec(&(
            path_key,
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
    entries.sort_by(|a, b| a.relative.cmp(&b.relative));
    let mut fingerprint = String::new();
    for e in &entries {
        fingerprint.push_str(&e.fingerprint);
    }
    let tree = digest(fingerprint.as_bytes());
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
        source.check()?;
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
            if regex.is_match(&entry.name) {
                let m = FileSearchMatch {
                    path: entry.path.clone(),
                    kind: if entry.directory { "directory" } else { "file" }.into(),
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
        if !entry.regular {
            continue;
        }
        if entry.size > *max_file_bytes as u64 {
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
        let loaded =
            tokio::time::timeout_at(deadline, source.load(&entry.path, *max_file_bytes as usize))
                .await;
        let bytes = match loaded {
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
        let hash = digest(&bytes);
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
        let doc = match decode(&bytes, None) {
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
            source.check()?;
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
                    path: entry.path.clone(),
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
pub fn warn(s: &mut FileSearchSummary, path: &str, reason: &str) {
    s.skipped_entries += 1;
    if s.warnings.len() < 8 {
        s.warnings
            .push(format!("{path}: {reason}").chars().take(160).collect());
    }
}
pub fn stop(s: &mut FileSearchSummary, reason: &str) {
    s.truncated = true;
    if s.stop_reason.is_none() {
        s.stop_reason = Some(reason.into());
    }
}
