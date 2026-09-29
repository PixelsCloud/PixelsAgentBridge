use std::path::{Path, PathBuf};

use pab_protocol::{DeviceRef, MAX_SCREENSHOT_BYTES, RequestId};
use sqlx::Row;
use tokio::io::AsyncWriteExt;

use crate::connection::{BridgeError, Screenshot};

use super::store::{RuntimeStore, RuntimeStoreError};
use super::{BridgeRuntime, RuntimeError};

pub(super) fn screenshot_dir(database_path: &Path) -> PathBuf {
    database_path.with_extension("screenshots")
}

pub(super) async fn read_screenshot(
    store: &RuntimeStore,
    directory: &Path,
    id: &str,
) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Ok(None);
    }
    let row = sqlx::query(
        "SELECT size FROM runtime_operations WHERE id = ? AND kind = 'screenshot' AND state = 'completed'",
    )
    .bind(id)
    .fetch_optional(&store.pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let expected_size: i64 = row.try_get("size")?;
    let path = directory.join(format!("{id}.png"));
    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(RuntimeStoreError::Io(error)),
    };
    if metadata.len() == 0
        || metadata.len() > MAX_SCREENSHOT_BYTES as u64
        || i64::try_from(metadata.len()).ok() != Some(expected_size)
    {
        return Ok(None);
    }
    let bytes = tokio::fs::read(path).await.map_err(RuntimeStoreError::Io)?;
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() as u64 == metadata.len() {
        Ok(Some(bytes))
    } else {
        Ok(None)
    }
}

async fn write_screenshot(path: &Path, bytes: &[u8]) -> Result<(), BridgeError> {
    if let Some(parent) = path.parent() {
        pab_agent_core::ensure_data_dir(parent)
            .map_err(|error| BridgeError::FileTransfer(error.to_string()))?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .map_err(|error| BridgeError::FileTransfer(error.to_string()))?;
    if let Err(error) = async {
        file.write_all(bytes).await?;
        file.sync_all().await
    }
    .await
    {
        drop(file);
        let _ = tokio::fs::remove_file(path).await;
        return Err(BridgeError::FileTransfer(error.to_string()));
    }
    Ok(())
}

impl BridgeRuntime {
    pub async fn preview_screenshot(
        &self,
        device_ref: DeviceRef,
    ) -> Result<Screenshot, RuntimeError> {
        let connection = self.inner.device(device_ref).await.connection().await?;
        connection
            .capture_screenshot(RequestId::new())
            .await
            .map_err(RuntimeError::from)
    }

    pub async fn capture_screenshot(
        &self,
        device_ref: DeviceRef,
        destination: Option<&Path>,
    ) -> Result<Screenshot, RuntimeError> {
        if destination.is_some_and(|path| !path.is_absolute() || path.file_name().is_none()) {
            return Err(RuntimeError::Bridge(BridgeError::FileTransfer(
                "screenshot destination must be an absolute file path".to_owned(),
            )));
        }
        let id = RequestId::new();
        let archive = self.inner.screenshot_dir.join(format!("{id}.png"));
        let destination_label = destination
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| archive.to_string_lossy().into_owned());
        let device_code = self
            .inner
            .device_codes
            .lock()
            .await
            .get(&device_ref)
            .copied();
        self.inner
            .store
            .start_screenshot_operation(
                id,
                device_ref,
                device_code,
                &self.inner.initiated_by,
                &destination_label,
                &self.inner.session_id,
            )
            .await?;
        let result = async {
            let connection = self.inner.device(device_ref).await.connection().await?;
            let image = connection.capture_screenshot(id).await?;
            write_screenshot(&archive, &image.bytes).await?;
            if let Some(path) = destination {
                if let Err(error) = write_screenshot(path, &image.bytes).await {
                    let _ = tokio::fs::remove_file(&archive).await;
                    return Err(RuntimeError::Bridge(error));
                }
            }
            Ok(image)
        }
        .await;
        if let Ok(image) = &result {
            self.inner
                .store
                .operation_progress(id, image.meta.size, image.meta.size)
                .await?;
        }
        let (state, message) = match &result {
            Ok(_) => ("completed", None),
            Err(error) => ("failed", Some(error.to_string())),
        };
        self.inner
            .store
            .finish_operation(id, state, message.as_deref())
            .await?;
        result
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn screenshot_history_reads_only_completed_matching_files() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("bridge.sqlite3");
        let store = RuntimeStore::open(&database).await.unwrap();
        let id = RequestId::new().to_string();
        let image = b"\x89PNG\r\n\x1a\nimage";
        let archive = screenshot_dir(&database).join(format!("{id}.png"));
        write_screenshot(&archive, image).await.unwrap();
        sqlx::query("INSERT INTO runtime_operations (id, device_ref_json, kind, direction, source, destination, overwrite, state, size, started_at_unix_ms) VALUES (?, '{}', 'screenshot', 'download', '', '', 0, 'completed', ?, 1)")
            .bind(&id)
            .bind(image.len() as i64)
            .execute(&store.pool)
            .await
            .unwrap();

        store.close().await;
        let reopened = super::super::BridgeLocalStore::open(&database)
            .await
            .unwrap();
        assert_eq!(
            reopened.screenshot_bytes(&id).await.unwrap(),
            Some(image.to_vec())
        );
        assert!(reopened.screenshot_bytes("../bad").await.unwrap().is_none());
        sqlx::query("UPDATE runtime_operations SET state = 'failed' WHERE id = ?")
            .bind(&id)
            .execute(&reopened.store.pool)
            .await
            .unwrap();
        assert!(reopened.screenshot_bytes(&id).await.unwrap().is_none());
    }
}
