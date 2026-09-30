use serde::{Deserialize, Serialize};

use crate::RequestId;

pub const MAX_TEXT_FILE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TEXT_PAYLOAD_BYTES: usize = 128 * 1024;
pub const MAX_TEXT_READ_BYTES: usize = 16 * 1024;
pub const MAX_TEXT_EDITS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum TextReadRange {
    Bytes { offset: u64, max_bytes: u32 },
    Lines { start_line: u64, count: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileSystemAction {
    Copy {
        destination: String,
        recursive: bool,
        overwrite: bool,
        limits: FileOperationLimits,
    },
    Move {
        destination: String,
        recursive: bool,
        overwrite: bool,
        limits: FileOperationLimits,
    },
    Delete {
        recursive: bool,
        limits: FileOperationLimits,
    },
    ArchiveCreate {
        sources: Vec<String>,
        overwrite: bool,
        limits: FileOperationLimits,
    },
    ArchiveExtract {
        destination: String,
        overwrite: bool,
        max_ratio: u32,
        limits: FileOperationLimits,
    },
    Search {
        mode: FileSearchMode,
        query: String,
        glob: String,
        case_sensitive: bool,
        max_results: u32,
        max_depth: u32,
        max_file_bytes: u32,
    },
    Hash,
    Mkdir {
        parents: bool,
        exist_ok: bool,
    },
    Stat {
        follow_symlinks: bool,
    },
    Read {
        range: TextReadRange,
        encoding: Option<TextEncoding>,
        expected_hash: Option<String>,
    },
    Write {
        encoding: TextEncoding,
        overwrite: bool,
        expected_hash: Option<String>,
    },
    Patch {
        expected_hash: String,
        encoding: Option<TextEncoding>,
    },
}

impl FileSystemAction {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Copy { .. } => "file_copy",
            Self::Move { .. } => "file_move",
            Self::Delete { .. } => "file_delete",
            Self::ArchiveCreate { .. } => "archive_create",
            Self::ArchiveExtract { .. } => "archive_extract",
            Self::Search { .. } => "file_search",
            Self::Hash => "file_hash",
            Self::Mkdir { .. } => "mkdir",
            Self::Stat { .. } => "file_stat",
            Self::Read { .. } => "file_read",
            Self::Write { .. } => "file_write",
            Self::Patch { .. } => "file_patch",
        }
    }

    pub fn destination(&self) -> Option<&str> {
        match self {
            Self::Copy { destination, .. }
            | Self::Move { destination, .. }
            | Self::ArchiveExtract { destination, .. } => Some(destination),
            _ => None,
        }
    }

    pub const fn is_bulk(&self) -> bool {
        matches!(
            self,
            Self::Copy { .. }
                | Self::Move { .. }
                | Self::Delete { .. }
                | Self::ArchiveCreate { .. }
                | Self::ArchiveExtract { .. }
        )
    }
    pub const fn asynchronous(&self) -> bool {
        matches!(self, Self::Hash) || self.is_bulk()
    }

    pub const fn has_payload(&self) -> bool {
        matches!(self, Self::Write { .. } | Self::Patch { .. })
    }

    pub const fn schema_version(&self) -> u16 {
        match self {
            Self::Copy { .. }
            | Self::Move { .. }
            | Self::Delete { .. }
            | Self::ArchiveCreate { .. }
            | Self::ArchiveExtract { .. } => 3,
            Self::Search { .. } | Self::Hash | Self::Mkdir { .. } => 2,
            _ => 1,
        }
    }

    pub const fn mutates(&self) -> bool {
        self.is_bulk()
            || matches!(
                self,
                Self::Write { .. } | Self::Patch { .. } | Self::Mkdir { .. }
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSystemRequest {
    pub request_id: RequestId,
    pub path: String,
    pub operation: FileSystemAction,
    pub payload_size: u32,
    pub payload_sha256: Option<String>,
}

impl FileSystemRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.path.is_empty() || self.path.len() > 4096 || self.path.contains('\0') {
            return Err("path must contain 1..4096 bytes without NUL");
        }
        if self.payload_size as usize > MAX_TEXT_PAYLOAD_BYTES {
            return Err("text payload exceeds 128 KiB; use file transfer for larger content");
        }
        if self.operation.has_payload() {
            if !self.payload_sha256.as_deref().is_some_and(valid_file_hash) {
                return Err("write/patch require a SHA-256 for their binary payload");
            }
        } else if self.payload_size != 0 || self.payload_sha256.is_some() {
            return Err("this operation does not accept an input payload");
        }
        if self
            .operation
            .destination()
            .is_some_and(|path| path.is_empty() || path.len() > 4096 || path.contains('\0'))
        {
            return Err("destination requires 1..4096 bytes without NUL");
        }
        match &self.operation {
            FileSystemAction::Copy { limits, .. }
            | FileSystemAction::Move { limits, .. }
            | FileSystemAction::Delete { limits, .. } => limits.validate()?,
            FileSystemAction::ArchiveExtract {
                limits, max_ratio, ..
            } => {
                limits.validate()?;
                if !(1..=1000).contains(max_ratio) {
                    return Err("max_ratio must be 1..1000");
                }
            }
            FileSystemAction::ArchiveCreate {
                sources, limits, ..
            } => {
                limits.validate()?;
                if sources.is_empty()
                    || sources.len() > 32
                    || sources
                        .iter()
                        .any(|path| path.is_empty() || path.len() > 4096 || path.contains('\0'))
                    || sources.iter().map(String::len).sum::<usize>() > 16 * 1024
                {
                    return Err(
                        "sources require 1..32 paths, at most 4096 bytes each and 16 KiB total",
                    );
                }
            }
            FileSystemAction::Search {
                query,
                glob,
                max_results,
                max_depth,
                max_file_bytes,
                ..
            } => {
                if query.is_empty() || query.len() > 1024 || query.contains(['\0', '\n', '\r']) {
                    return Err("query requires 1..1024 bytes without NUL or newlines");
                }
                if glob.is_empty() || glob.len() > 1024 || glob.contains('\0') {
                    return Err("glob requires 1..1024 bytes without NUL");
                }
                if !(1..=100).contains(max_results)
                    || !(1..=64).contains(max_depth)
                    || !(1..=MAX_TEXT_FILE_BYTES as u32).contains(max_file_bytes)
                {
                    return Err(
                        "search limits: 1..100 matches, 1..64 depth, 1..4194304 file bytes",
                    );
                }
            }
            FileSystemAction::Read {
                range,
                expected_hash,
                ..
            } => {
                match range {
                    TextReadRange::Bytes { max_bytes, .. }
                        if !(4..=MAX_TEXT_READ_BYTES as u32).contains(max_bytes) =>
                    {
                        return Err("max_bytes must be 4..16384");
                    }
                    TextReadRange::Lines { start_line, count }
                        if *start_line == 0 || !(1..=500).contains(count) =>
                    {
                        return Err("start_line is 1-based; count must be 1..500");
                    }
                    _ => {}
                }
                if expected_hash
                    .as_deref()
                    .is_some_and(|hash| !valid_file_hash(hash))
                {
                    return Err("expected_hash must be lowercase SHA-256");
                }
            }
            FileSystemAction::Write {
                expected_hash,
                overwrite,
                ..
            } => {
                if expected_hash.is_some() && !overwrite {
                    return Err("expected_hash requires overwrite=true");
                }
                if expected_hash
                    .as_deref()
                    .is_some_and(|hash| !valid_file_hash(hash))
                {
                    return Err("expected_hash must be lowercase SHA-256");
                }
            }
            FileSystemAction::Patch { expected_hash, .. } if !valid_file_hash(expected_hash) => {
                return Err("patch requires the original lowercase SHA-256");
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn valid_file_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextEdit {
    pub find: String,
    pub replace: String,
    pub expected_matches: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMetadata {
    pub kind: String,
    pub size: u64,
    pub modified_at_unix_ms: Option<i64>,
    pub readonly: bool,
    pub unix_mode: Option<u32>,
    pub is_link: bool,
    pub link_target: Option<String>,
    pub sha256: Option<String>,
    pub encoding: Option<TextEncoding>,
    pub newline: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextReadPosition {
    pub offset: u64,
    pub next_offset: u64,
    pub next_line: Option<u64>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSystemError {
    pub code: String,
    pub phase: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSystemReply {
    pub request_id: RequestId,
    pub path: String,
    pub kind: String,
    pub state: String,
    pub metadata: Option<FileMetadata>,
    pub range: Option<TextReadPosition>,
    pub error: Option<FileSystemError>,
    pub data_size: u32,
    pub data_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<FileSearchSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<FileHashProgress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_paths: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation: Option<FileMutationSummary>,
}

impl FileSystemReply {
    pub fn pending(request: &FileSystemRequest) -> Self {
        Self {
            request_id: request.request_id,
            path: request.path.clone(),
            kind: request.operation.kind().to_owned(),
            state: "running".to_owned(),
            metadata: None,
            range: None,
            error: None,
            data_size: 0,
            data_sha256: None,
            search: None,
            progress: None,
            created_paths: None,
            destination: request.operation.destination().map(str::to_owned),
            mutation: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSearchMode {
    Name,
    Content,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchMatch {
    pub path: String,
    pub kind: String,
    pub line: Option<u64>,
    pub preview: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileSearchSummary {
    pub matches: Vec<FileSearchMatch>,
    pub scanned_entries: u32,
    pub scanned_bytes: u64,
    pub skipped_entries: u32,
    pub truncated: bool,
    pub stop_reason: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHashProgress {
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub updated_at_unix_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_uses_byte_limits_and_non_payload_mutations_are_explicit() {
        let mut request = FileSystemRequest {
            request_id: RequestId::new(),
            path: "/tmp/root".to_owned(),
            operation: FileSystemAction::Search {
                mode: FileSearchMode::Content,
                query: "中".repeat(342),
                glob: "**/*".to_owned(),
                case_sensitive: true,
                max_results: 100,
                max_depth: 64,
                max_file_bytes: MAX_TEXT_FILE_BYTES as u32,
            },
            payload_size: 0,
            payload_sha256: None,
        };
        assert!(request.validate().is_err());
        if let FileSystemAction::Search { query, .. } = &mut request.operation {
            *query = "a".repeat(1024);
        }
        assert!(request.validate().is_ok());
        request.payload_size = 1;
        assert!(request.validate().is_err());
        request.payload_size = 0;
        request.operation = FileSystemAction::Mkdir {
            parents: false,
            exist_ok: false,
        };
        assert!(request.operation.mutates());
        assert!(!request.operation.has_payload());
        assert!(request.validate().is_ok());
        assert_eq!(request.operation.schema_version(), 2);
        request.operation = FileSystemAction::Stat {
            follow_symlinks: false,
        };
        assert_eq!(request.operation.schema_version(), 1);
        request.operation = FileSystemAction::Hash;
        request.payload_sha256 = Some("a".repeat(64));
        assert!(request.validate().is_err());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileOperationLimits {
    pub max_entries: u32,
    pub max_bytes: u64,
    pub max_depth: u32,
}
impl Default for FileOperationLimits {
    fn default() -> Self {
        Self {
            max_entries: 4096,
            max_bytes: 1024 * 1024 * 1024,
            max_depth: 64,
        }
    }
}
impl FileOperationLimits {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(1..=4096).contains(&self.max_entries)
            || self.max_bytes > 8 * 1024 * 1024 * 1024
            || !(1..=64).contains(&self.max_depth)
        {
            Err("limits: 1..4096 entries, 0..8 GiB bytes, 1..64 depth")
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileItemResult {
    pub path: String,
    pub action: String,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileMutationSummary {
    pub phase: String,
    pub total_entries: u32,
    pub processed_entries: u32,
    pub published_entries: u32,
    pub deleted_entries: u32,
    pub partial: bool,
    pub source_removed: bool,
    pub results: Vec<FileItemResult>,
    pub results_truncated: bool,
}

pub fn cancellable_filesystem_kind(kind: &str) -> bool {
    matches!(
        kind,
        "file_hash"
            | "file_copy"
            | "file_move"
            | "file_delete"
            | "archive_create"
            | "archive_extract"
    )
}
