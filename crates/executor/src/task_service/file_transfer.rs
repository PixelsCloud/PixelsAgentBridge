//! Service-account adapter; native user workers use the same transfer engine.
use super::{TaskServiceError, transfer_engine::TransferEngine, upload_lock::UploadPathLocks};
use crate::task_store::TaskStore;
use pab_protocol::RequestId;
use pab_transport::PabBiStream;
use std::time::Duration;

pub(super) async fn upload(
    store: &TaskStore,
    request_id: RequestId,
    stream: &mut PabBiStream,
    timeout: Duration,
    path: &str,
    size: u64,
    sha256: &str,
    overwrite: bool,
    upload_locks: &UploadPathLocks,
) -> Result<(), TaskServiceError> {
    TransferEngine::local(store, request_id, upload_locks)
        .upload(stream, timeout, path, size, sha256, overwrite)
        .await
}

pub(super) async fn download(
    store: &TaskStore,
    request_id: RequestId,
    stream: &mut PabBiStream,
    timeout: Duration,
    path: &str,
    offset: u64,
) -> Result<(), TaskServiceError> {
    TransferEngine::local(store, request_id, &UploadPathLocks::default())
        .download(stream, timeout, path, offset, None)
        .await
}
