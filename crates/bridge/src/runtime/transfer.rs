use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use pab_protocol::{DeviceRef, RequestId};

use super::{BridgeRuntime, RuntimeError};

impl BridgeRuntime {
    /// Execute a submission that has already been durably accepted by TransferQueue.
    /// The queue, not this adapter, decides the final observation after a failure.
    pub async fn execute_queued_transfer(
        &self,
        spec: &super::TransferRequest,
        control: &crate::TransferControl,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), RuntimeError> {
        let resolved = self.resolve_cached_device_code(spec.device_code).await?;
        if resolved != spec.device_ref {
            return Err(RuntimeError::TaskOperation(
                "device code now identifies another device".to_owned(),
            ));
        }
        let connection = self
            .inner
            .device(spec.device_ref)
            .await
            .connection()
            .await?;
        let expected_hash = if let Some(original) = spec.options.resume_from {
            let (_, context) = self.inner.store.transfer_context(original).await?;
            control
                .expect_context(context.ok_or(super::RuntimeStoreError::SnapshotIdentityMismatch)?);
            let hash: Option<String> =
                sqlx::query_scalar("SELECT sha256 FROM runtime_async_transfers WHERE id=?")
                    .bind(original.to_string())
                    .fetch_optional(&self.inner.store.pool)
                    .await
                    .map_err(super::RuntimeStoreError::from)?
                    .flatten();
            if spec.direction == "download" && hash.is_none() {
                return Err(RuntimeError::TaskOperation(
                    "original download hash is unavailable".into(),
                ));
            }
            hash
        } else {
            None
        };
        if spec.direction == "upload" {
            connection
                .upload_file_with_options(
                    spec.request_id,
                    std::path::Path::new(&spec.source),
                    &spec.destination,
                    spec.overwrite,
                    &spec.options,
                    control,
                    progress,
                )
                .await?;
        } else {
            connection
                .download_file_with_options(
                    spec.request_id,
                    &spec.source,
                    std::path::Path::new(&spec.destination),
                    spec.overwrite,
                    &spec.options,
                    expected_hash.as_deref(),
                    control,
                    progress,
                )
                .await?;
        }
        Ok(())
    }

    pub async fn prepare_transfer_record(
        &self,
        id: RequestId,
        device_ref: DeviceRef,
        direction: &str,
        source: &str,
        destination: &str,
        overwrite: bool,
    ) -> Result<(), RuntimeError> {
        if !matches!(direction, "upload" | "download") {
            return Err(RuntimeError::BridgeUnavailable(
                "invalid transfer direction".to_owned(),
            ));
        }
        let device_code = self
            .inner
            .device_codes
            .lock()
            .await
            .get(&device_ref)
            .copied();
        self.inner
            .store
            .start_operation(
                id,
                device_ref,
                device_code,
                &self.inner.initiated_by,
                direction,
                source,
                destination,
                overwrite,
                Some(&self.inner.session_id),
            )
            .await?;
        Ok(())
    }

    pub async fn transfer_status(
        &self,
        device_ref: DeviceRef,
        request_id: RequestId,
    ) -> Result<pab_protocol::TransferSnapshot, RuntimeError> {
        let device = self.inner.device(device_ref).await;
        loop {
            let connection = device.connection().await?;
            match connection.get_transfer(request_id).await {
                Ok(snapshot) => return Ok(snapshot),
                Err(error) if error.is_recoverable_connection() => {
                    device.recover(&connection, &error).await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub async fn upload_file(
        &self,
        device_ref: DeviceRef,
        source: &std::path::Path,
        destination: &str,
        overwrite: bool,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), RuntimeError> {
        self.upload_file_with_id(
            RequestId::new(),
            device_ref,
            source,
            destination,
            overwrite,
            progress,
        )
        .await
    }

    pub async fn upload_file_with_id(
        &self,
        id: RequestId,
        device_ref: DeviceRef,
        source: &std::path::Path,
        destination: &str,
        overwrite: bool,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), RuntimeError> {
        let progress_store = self.inner.store.clone();
        let final_progress = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
        let callback_progress = Arc::clone(&final_progress);
        self.transfer_file(
            id,
            device_ref,
            "upload",
            &source.to_string_lossy(),
            destination,
            overwrite,
            final_progress,
            async {
                let connection = self.inner.device(device_ref).await.connection().await?;
                connection
                    .upload_file(id, source, destination, overwrite, move |offset, size| {
                        progress(offset, size);
                        callback_progress.0.store(offset, Ordering::Release);
                        callback_progress.1.store(size, Ordering::Release);
                        let store = progress_store.clone();
                        tokio::spawn(async move {
                            if let Err(error) = store.operation_progress(id, offset, size).await {
                                tracing::warn!(%error, "failed to save transfer progress");
                            }
                        });
                    })
                    .await
                    .map_err(Into::into)
            },
        )
        .await
    }

    pub async fn download_file(
        &self,
        device_ref: DeviceRef,
        source: &str,
        destination: &std::path::Path,
        overwrite: bool,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), RuntimeError> {
        self.download_file_with_id(
            RequestId::new(),
            device_ref,
            source,
            destination,
            overwrite,
            progress,
        )
        .await
    }

    pub async fn download_file_with_id(
        &self,
        id: RequestId,
        device_ref: DeviceRef,
        source: &str,
        destination: &std::path::Path,
        overwrite: bool,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), RuntimeError> {
        let progress_store = self.inner.store.clone();
        let final_progress = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
        let callback_progress = Arc::clone(&final_progress);
        self.transfer_file(
            id,
            device_ref,
            "download",
            source,
            &destination.to_string_lossy(),
            overwrite,
            final_progress,
            async {
                let connection = self.inner.device(device_ref).await.connection().await?;
                connection
                    .download_file(id, source, destination, overwrite, move |offset, size| {
                        progress(offset, size);
                        callback_progress.0.store(offset, Ordering::Release);
                        callback_progress.1.store(size, Ordering::Release);
                        let store = progress_store.clone();
                        tokio::spawn(async move {
                            if let Err(error) = store.operation_progress(id, offset, size).await {
                                tracing::warn!(%error, "failed to save transfer progress");
                            }
                        });
                    })
                    .await
                    .map_err(Into::into)
            },
        )
        .await
    }

    async fn transfer_file<F>(
        &self,
        id: RequestId,
        device_ref: DeviceRef,
        direction: &str,
        source: &str,
        destination: &str,
        overwrite: bool,
        final_progress: Arc<(AtomicU64, AtomicU64)>,
        operation: F,
    ) -> Result<(), RuntimeError>
    where
        F: std::future::Future<Output = Result<(), RuntimeError>>,
    {
        self.prepare_transfer_record(id, device_ref, direction, source, destination, overwrite)
            .await?;
        let result = operation.await;
        self.inner
            .store
            .operation_progress(
                id,
                final_progress.0.load(Ordering::Acquire),
                final_progress.1.load(Ordering::Acquire),
            )
            .await?;
        let (state, message) = match &result {
            Ok(()) => ("completed", None),
            Err(error) => ("failed", Some(error.to_string())),
        };
        self.inner
            .store
            .finish_operation(id, state, message.as_deref())
            .await?;
        result
    }

    pub async fn cancel_transfer_record(&self, id: RequestId) -> Result<bool, RuntimeError> {
        let recorded = self
            .inner
            .store
            .request_transfer_cancellation(id, &self.inner.session_id)
            .await?;
        if recorded {
            self.inner.reconciliation_notify.notify_one();
        }
        Ok(recorded)
    }
}
