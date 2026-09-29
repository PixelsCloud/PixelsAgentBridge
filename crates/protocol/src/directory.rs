use serde::{Deserialize, Serialize};

use crate::RequestId;

pub const MAX_DIRECTORY_PATH_BYTES: usize = 4 * 1024;
pub const MAX_DIRECTORY_NAME_BYTES: usize = 255;
pub const MAX_DIRECTORY_PAGE_ENTRIES: u16 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectoryEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub kind: DirectoryEntryKind,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryPage {
    pub request_id: RequestId,
    pub path: String,
    pub entries: Vec<DirectoryEntry>,
    pub next_after: Option<String>,
}
