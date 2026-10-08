use std::path::{Path, PathBuf};

use pab_protocol::{
    DeviceRef, MAX_SCREENSHOT_BYTES, RequestId, ScreenshotFormat, ScreenshotMeta, ScreenshotOptions,
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
        "SELECT size, meta_json FROM runtime_operations LEFT JOIN runtime_screenshot_results USING (id) WHERE id = ? AND kind = 'screenshot' AND state = 'completed'",
    )
    .bind(id)
    .fetch_optional(&store.pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let expected_size: i64 = row.try_get("size")?;
    let meta: Option<ScreenshotMeta> = row
        .try_get::<Option<String>, _>("meta_json")?
        .map(|json| serde_json::from_str(&json))
        .transpose()?;
    if meta.as_ref().is_some_and(|meta| {
        meta.request_id.to_string() != id
            || meta
                .capture
                .as_ref()
                .is_none_or(|info| (info.width, info.height) != (meta.width, meta.height))
    }) {
        return Ok(None);
    }
    let extension = match meta.as_ref().map(|m| m.format.as_str()) {
        None | Some("png") => "png",
        Some("jpeg") => "jpg",
        _ => return Ok(None),
    };
    let path = directory.join(format!("{id}.{extension}"));
    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(RuntimeStoreError::Io(error)),
    };
    let native_jpeg = meta
        .as_ref()
        .and_then(|m| m.capture.as_ref())
        .is_some_and(|i| i.mode == pab_protocol::ScreenshotMode::Jpeg);
    if metadata.len() == 0
        || (!native_jpeg && metadata.len() > MAX_SCREENSHOT_BYTES as u64)
        || i64::try_from(metadata.len()).ok() != Some(expected_size)
    {
        return Ok(None);
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(RuntimeStoreError::Io)?;
    let mut bytes = Vec::new();
    file.take(metadata.len().saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(RuntimeStoreError::Io)?;
    if let Some(meta) = meta {
        return tokio::task::spawn_blocking(move || {
            if bytes.len() as u64 != meta.size
                || format!("{:x}", Sha256::digest(&bytes)) != meta.sha256
            {
                return None;
            }
            let info = meta.capture?;
            let options = ScreenshotOptions {
                window_ref: info.window_ref.clone(),
                mode: info.mode,
                format: Some(info.format),
                max_bytes: (info.mode != pab_protocol::ScreenshotMode::Jpeg)
                    .then_some(MAX_SCREENSHOT_BYTES as u32),
                max_width: (info.mode == pab_protocol::ScreenshotMode::Preview).then_some(8192),
                max_height: (info.mode == pab_protocol::ScreenshotMode::Preview).then_some(8192),
                quality: info.quality,
                ..Default::default()
            };
            pab_screenshot::verify(&bytes, &info, &options).ok()?;
            Some(bytes)
        })
        .await
        .map_err(|error| RuntimeStoreError::Io(std::io::Error::other(error.to_string())));
    }
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
            .capture_screenshot_with_options(RequestId::new(), &ScreenshotOptions::default())
            .await
            .map_err(RuntimeError::from)
    }

    pub async fn capture_screenshot(
        &self,
        device_ref: DeviceRef,
        destination: Option<&Path>,
    ) -> Result<Screenshot, RuntimeError> {
        self.capture_screenshot_inner(device_ref, destination, None)
            .await
    }

    pub async fn capture_screenshot_with_options(
        &self,
        device_ref: DeviceRef,
        destination: Option<&Path>,
        options: &ScreenshotOptions,
    ) -> Result<Screenshot, RuntimeError> {
        options
            .validate()
            .map_err(|e| RuntimeError::Bridge(BridgeError::FileTransfer(e.into())))?;
        if destination.is_some_and(|path| {
            let extension = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            match options.format() {
                ScreenshotFormat::Png => extension != "png",
                ScreenshotFormat::Jpeg => !matches!(extension.as_str(), "jpg" | "jpeg"),
            }
        }) {
            return Err(RuntimeError::Bridge(BridgeError::FileTransfer(
                "destination extension must match screenshot format (.png or .jpg/.jpeg)".into(),
            )));
        }
        self.capture_screenshot_inner(device_ref, destination, Some(options))
            .await
    }

    pub fn screenshot_archive_path(&self, id: RequestId, format: ScreenshotFormat) -> PathBuf {
        self.inner
            .screenshot_dir
            .join(format!("{id}.{}", format.extension()))
    }

    async fn capture_screenshot_inner(
        &self,
        device_ref: DeviceRef,
        destination: Option<&Path>,
        options: Option<&ScreenshotOptions>,
    ) -> Result<Screenshot, RuntimeError> {
        if destination.is_some_and(|path| !path.is_absolute() || path.file_name().is_none()) {
            return Err(RuntimeError::Bridge(BridgeError::FileTransfer(
                "screenshot destination must be an absolute file path".to_owned(),
            )));
        }
        if let Some(path) = destination {
            match tokio::fs::symlink_metadata(path).await {
                Ok(_) => {
                    return Err(RuntimeError::Bridge(BridgeError::FileTransfer(
                        "screenshot destination already exists; files are never overwritten".into(),
                    )));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(RuntimeError::Bridge(BridgeError::FileTransfer(
                        e.to_string(),
                    )));
                }
            }
        }
        let id = RequestId::new();
        let archive = self.screenshot_archive_path(
            id,
            options
                .map(ScreenshotOptions::format)
                .unwrap_or(ScreenshotFormat::Png),
        );
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
        self.inner.wait_account_ready().await?;
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
            let image = match options {
                Some(options) => {
                    connection
                        .capture_screenshot_with_options(id, options)
                        .await?
                }
                None => connection.capture_screenshot(id).await?,
            };
            write_screenshot(&archive, &image.bytes).await?;
            if let Some(path) = destination
                && let Err(error) = write_screenshot(path, &image.bytes).await
            {
                let _ = tokio::fs::remove_file(&archive).await;
                return Err(RuntimeError::Bridge(error));
            }
            if image.meta.capture.is_some() {
                sqlx::query("INSERT INTO runtime_screenshot_results (id, meta_json) VALUES (?, ?)")
                    .bind(id.to_string())
                    .bind(serde_json::to_string(&image.meta).map_err(RuntimeStoreError::from)?)
                    .execute(&self.inner.store.pool)
                    .await
                    .map_err(RuntimeStoreError::from)?;
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
    async fn large_native_jpeg_history_survives_reopen_and_rejects_tampering() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("bridge.sqlite3");
        let store = RuntimeStore::open(&database).await.unwrap();
        let id = RequestId::new();
        let options = ScreenshotOptions::default();
        let mut seed = 17u32;
        let pixels = pab_screenshot::image::RgbImage::from_fn(3840, 2160, |_, _| {
            let mut pixel = [0u8; 3];
            for value in &mut pixel {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *value = seed as u8;
            }
            pab_screenshot::image::Rgb(pixel)
        });
        let encoded = pab_screenshot::encode(
            pab_screenshot::image::DynamicImage::ImageRgb8(pixels),
            &options,
            Some(1),
            (0, 0),
        )
        .unwrap();
        let image = encoded.bytes;
        assert!(image.len() > pab_protocol::MAX_SCREENSHOT_BYTES);
        let meta = ScreenshotMeta {
            request_id: id,
            format: "jpeg".into(),
            width: 3840,
            height: 2160,
            size: image.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&image)),
            capture: Some(encoded.info),
        };
        let archive = screenshot_dir(&database).join(format!("{id}.jpg"));
        write_screenshot(&archive, &image).await.unwrap();
        sqlx::query("INSERT INTO runtime_operations (id, device_ref_json, kind, direction, source, destination, overwrite, state, size, started_at_unix_ms) VALUES (?, '{}', 'screenshot', 'download', '', '', 0, 'completed', ?, 1)")
            .bind(id.to_string()).bind(image.len() as i64).execute(&store.pool).await.unwrap();
        sqlx::query("INSERT INTO runtime_screenshot_results (id,meta_json) VALUES (?,?)")
            .bind(id.to_string())
            .bind(serde_json::to_string(&meta).unwrap())
            .execute(&store.pool)
            .await
            .unwrap();
        store.close().await;
        let reopened = super::super::BridgeLocalStore::open(&database)
            .await
            .unwrap();
        assert_eq!(
            reopened.screenshot_bytes(&id.to_string()).await.unwrap(),
            Some(image.clone())
        );
        let mut changed = image;
        changed[30] ^= 1;
        tokio::fs::write(&archive, &changed).await.unwrap();
        assert!(
            reopened
                .screenshot_bytes(&id.to_string())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn screenshot_destination_never_overwrites_an_existing_file() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("image.jpg");
        write_screenshot(&path, b"existing").await.unwrap();
        assert!(write_screenshot(&path, b"replacement").await.is_err());
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"existing");
    }

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
