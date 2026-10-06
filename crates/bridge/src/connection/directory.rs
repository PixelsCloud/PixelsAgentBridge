use pab_protocol::{
    DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, DeviceTaskResponse, DirectoryPage,
    MAX_DIRECTORY_NAME_BYTES, MAX_DIRECTORY_PAGE_ENTRIES, RequestId,
};

use super::{AuthenticatedDeviceConnection, BridgeError, unexpected_task_response};

impl AuthenticatedDeviceConnection {
    pub async fn list_directory(
        &self,
        request_id: RequestId,
        path: &str,
        after: Option<&str>,
        limit: u16,
    ) -> Result<DirectoryPage, BridgeError> {
        let response = self
            .task_request(DeviceTaskRequest::ListDirectory {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id,
                path: path.to_owned(),
                after: after.map(str::to_owned),
                limit,
            })
            .await?;
        match response {
            DeviceTaskResponse::Directory { page }
                if valid_directory_page(&page, request_id, path, after, limit) =>
            {
                Ok(page)
            }
            response => Err(unexpected_task_response(response)),
        }
    }
}

pub(super) fn valid_directory_page(
    page: &DirectoryPage,
    request_id: RequestId,
    path: &str,
    after: Option<&str>,
    limit: u16,
) -> bool {
    if page.request_id != request_id
        || page.path != path
        || limit == 0
        || limit > MAX_DIRECTORY_PAGE_ENTRIES
        || page.entries.len() > usize::from(limit)
    {
        return false;
    }
    let mut previous = after.unwrap_or("");
    for entry in &page.entries {
        if entry.name.is_empty()
            || entry.name.len() > MAX_DIRECTORY_NAME_BYTES
            || entry.name.as_str() <= previous
            || entry.name.contains('/')
            || entry.name.contains('\\')
        {
            return false;
        }
        previous = &entry.name;
    }
    match page.next_after.as_deref() {
        None => true,
        Some(cursor) => page.entries.len() == usize::from(limit) && cursor == previous,
    }
}

#[cfg(test)]
mod tests {
    use pab_protocol::{DirectoryEntry, DirectoryEntryKind};

    use super::*;

    #[test]
    fn directory_response_must_match_request_and_sorted_page() {
        let id = RequestId::from_u128(1);
        let mut page = DirectoryPage {
            execution_context: None,
            request_id: id,
            path: "/tmp".to_owned(),
            entries: vec![DirectoryEntry {
                name: "a.txt".to_owned(),
                kind: DirectoryEntryKind::File,
                size: Some(1),
            }],
            next_after: Some("a.txt".to_owned()),
        };
        assert!(valid_directory_page(&page, id, "/tmp", None, 1));
        page.entries[0].name = "../a.txt".to_owned();
        assert!(!valid_directory_page(&page, id, "/tmp", None, 1));
    }
}
