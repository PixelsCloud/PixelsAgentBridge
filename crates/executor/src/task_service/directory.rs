use std::{collections::BTreeSet, path::Path};

use pab_protocol::{
    DirectoryEntry, DirectoryEntryKind, DirectoryPage, MAX_DIRECTORY_NAME_BYTES,
    MAX_DIRECTORY_PAGE_ENTRIES, MAX_DIRECTORY_PATH_BYTES, RequestId,
};
use tokio::fs;

use super::TaskServiceError;

const MAX_SCANNED_ENTRIES: usize = 20_000;

pub(super) async fn list(
    request_id: RequestId,
    path: String,
    after: Option<String>,
    limit: u16,
) -> Result<DirectoryPage, TaskServiceError> {
    if path.is_empty() || path.len() > MAX_DIRECTORY_PATH_BYTES || !Path::new(&path).is_absolute() {
        return Err(TaskServiceError::InvalidRequest(
            "directory path must be absolute and within the length limit",
        ));
    }
    if limit == 0 || limit > MAX_DIRECTORY_PAGE_ENTRIES {
        return Err(TaskServiceError::InvalidRequest(
            "directory page size is outside the supported range",
        ));
    }
    if after
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.len() > MAX_DIRECTORY_NAME_BYTES)
    {
        return Err(TaskServiceError::InvalidRequest(
            "directory cursor is invalid",
        ));
    }

    let mut reader = fs::read_dir(&path)
        .await
        .map_err(TaskServiceError::DirectoryRead)?;
    let mut names = BTreeSet::new();
    let mut scanned = 0;
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(TaskServiceError::DirectoryRead)?
    {
        scanned += 1;
        if scanned > MAX_SCANNED_ENTRIES {
            return Err(TaskServiceError::InvalidRequest(
                "directory contains too many entries",
            ));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| TaskServiceError::InvalidRequest("directory has a non-UTF-8 name"))?;
        if name.len() > MAX_DIRECTORY_NAME_BYTES {
            return Err(TaskServiceError::InvalidRequest(
                "directory has a name beyond the supported length",
            ));
        }
        if after
            .as_ref()
            .is_some_and(|cursor| name.as_str() <= cursor.as_str())
        {
            continue;
        }
        names.insert(name);
        if names.len() > usize::from(limit) + 1 {
            names.pop_last();
        }
    }

    let has_more = names.len() > usize::from(limit);
    if has_more {
        names.pop_last();
    }
    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        let metadata = fs::symlink_metadata(Path::new(&path).join(&name))
            .await
            .map_err(TaskServiceError::DirectoryRead)?;
        let kind = if metadata.file_type().is_symlink() {
            DirectoryEntryKind::Symlink
        } else if metadata.is_dir() {
            DirectoryEntryKind::Directory
        } else if metadata.is_file() {
            DirectoryEntryKind::File
        } else {
            DirectoryEntryKind::Other
        };
        entries.push(DirectoryEntry {
            name,
            kind,
            size: metadata.is_file().then_some(metadata.len()),
        });
    }
    let next_after = has_more.then(|| entries.last().unwrap().name.clone());
    Ok(DirectoryPage {
        request_id,
        path,
        entries,
        next_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pages_in_name_order_without_following_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["c.txt", "a.txt", "b.txt"] {
            tokio::fs::write(directory.path().join(name), name)
                .await
                .unwrap();
        }
        let path = directory.path().to_str().unwrap().to_owned();
        let first = list(RequestId::from_u128(1), path.clone(), None, 2)
            .await
            .unwrap();
        assert_eq!(
            first
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a.txt", "b.txt"]
        );
        assert_eq!(first.next_after.as_deref(), Some("b.txt"));
        let second = list(RequestId::from_u128(2), path, first.next_after, 2)
            .await
            .unwrap();
        assert_eq!(second.entries.len(), 1);
        assert_eq!(second.entries[0].name, "c.txt");
        assert!(second.next_after.is_none());
    }
}
