use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use pab_protocol::{DeviceTaskErrorCode, DeviceTaskResponse, RequestId};
use pab_transport::{MAX_BINARY_FRAME_BYTES, PabBiStream};
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

use super::{
    TaskServiceError,
    filesystem_engine::{FileCoordination, FileLocks},
    upload_lock::UploadPathLocks,
};
use crate::task_store::TaskStore;
use serde::{Deserialize, Serialize};
use std::{future::Future, io};
use tokio::sync::{mpsc, oneshot};

/// Network and local IPC adapters both implement this bounded streaming contract.
/// No adapter may turn file chunks into JSON or buffer a complete transfer.
pub(crate) trait TransferStream: Send {
    fn response(
        &mut self,
        value: &DeviceTaskResponse,
        end: bool,
        timeout: Duration,
    ) -> impl Future<Output = Result<(), TaskServiceError>> + Send;
    fn receive_binary_frame(
        &mut self,
        timeout: Duration,
    ) -> impl Future<Output = Result<Vec<u8>, TaskServiceError>> + Send;
    fn send_binary_frame(
        &mut self,
        bytes: &[u8],
        timeout: Duration,
    ) -> impl Future<Output = Result<(), TaskServiceError>> + Send;
}
impl TransferStream for PabBiStream {
    async fn response(
        &mut self,
        value: &DeviceTaskResponse,
        end: bool,
        timeout: Duration,
    ) -> Result<(), TaskServiceError> {
        if end {
            self.send_json(value, timeout).await?;
        } else {
            self.send_frame_json(value, timeout).await?;
        }
        Ok(())
    }
    async fn receive_binary_frame(
        &mut self,
        timeout: Duration,
    ) -> Result<Vec<u8>, TaskServiceError> {
        Ok(PabBiStream::receive_binary_frame(self, timeout).await?)
    }
    async fn send_binary_frame(
        &mut self,
        bytes: &[u8],
        timeout: Duration,
    ) -> Result<(), TaskServiceError> {
        Ok(PabBiStream::send_binary_frame(self, bytes, timeout).await?)
    }
}

/// Deliberately has no caller-supplied request ID. A worker's recorder is bound
/// by its parent to the original accepted transfer.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum TransferRecord {
    Progress { offset: u64, size: u64 },
    Hash { sha256: String },
    BeginPublication,
    PublicationFailed { message: String },
    Completed,
}
pub(crate) trait TransferRecorder: Send + Sync {
    fn record(&self, event: TransferRecord) -> impl Future<Output = io::Result<()>> + Send;
}
pub(crate) struct LocalRecorder {
    pub(crate) store: TaskStore,
    pub(crate) request_id: RequestId,
}
impl TransferRecorder for LocalRecorder {
    async fn record(&self, event: TransferRecord) -> io::Result<()> {
        let id = self.request_id;
        match event {
            TransferRecord::Progress { offset, size } if offset <= size => {
                self.store.transfer_progress(id, offset, size).await
            }
            TransferRecord::Hash { sha256 } if valid_sha256(&sha256) => {
                self.store.transfer_hash(id, &sha256).await
            }
            TransferRecord::BeginPublication => self.store.begin_transfer_publication(id).await,
            TransferRecord::PublicationFailed { message } if message.len() <= 8192 => {
                self.store.publication_failed(id, &message).await
            }
            TransferRecord::Completed => self.store.finish_transfer(id, "completed", None).await,
            _ => return Err(io::Error::other("invalid transfer record")),
        }
        .map_err(io::Error::other)
    }
}
pub(crate) struct TransferRecordRequest {
    pub(crate) event: TransferRecord,
    pub(crate) answer: oneshot::Sender<io::Result<()>>,
}
pub(crate) struct WorkerRecorder(pub(crate) mpsc::Sender<TransferRecordRequest>);
impl TransferRecorder for WorkerRecorder {
    async fn record(&self, event: TransferRecord) -> io::Result<()> {
        let (answer, result) = oneshot::channel();
        self.0
            .send(TransferRecordRequest { event, answer })
            .await
            .map_err(|_| io::Error::other("transfer coordinator disconnected"))?;
        result
            .await
            .map_err(|_| io::Error::other("transfer record unconfirmed"))?
    }
}

pub(crate) struct TransferEngine<R> {
    records: R,
    locks: FileLocks,
    resume_namespace: Option<String>,
}
impl TransferEngine<LocalRecorder> {
    pub(super) fn local(store: &TaskStore, id: RequestId, locks: &UploadPathLocks) -> Self {
        Self {
            records: LocalRecorder {
                store: store.clone(),
                request_id: id,
            },
            locks: FileLocks::Store(locks.clone()),
            resume_namespace: None,
        }
    }
}
impl<R: TransferRecorder> TransferEngine<R> {
    pub(crate) fn worker(
        records: R,
        locks: mpsc::Sender<FileCoordination>,
        identity: &[u8],
    ) -> Self {
        Self {
            records,
            locks: FileLocks::Worker(locks),
            resume_namespace: Some(format!("{:x}", Sha256::digest(identity))),
        }
    }
    pub(crate) async fn upload(
        &self,
        stream: &mut impl TransferStream,
        timeout: Duration,
        path: &str,
        size: u64,
        sha256: &str,
        overwrite: bool,
    ) -> Result<(), TaskServiceError> {
        let Some(destination) = valid_path(path) else {
            return reject(stream, timeout, "invalid destination path").await;
        };
        if !valid_sha256(sha256) {
            return reject(stream, timeout, "invalid checksum").await;
        }
        let Some(_upload_guard) = self.locks.try_acquire(&destination).await? else {
            return reject(stream, timeout, "destination already has an active upload").await;
        };
        match fs::symlink_metadata(&destination).await {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return reject(stream, timeout, "destination is not a regular file").await;
            }
            Ok(_) if !overwrite => {
                return reject(stream, timeout, "destination already exists").await;
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let partial = partial_path(&destination, sha256, self.resume_namespace.as_deref());
        if is_symlink(&partial).await? {
            return reject(stream, timeout, "partial file is a symbolic link").await;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&partial)
            .await?;
        let mut offset = file.metadata().await?.len();
        if offset > size {
            file.set_len(0).await?;
            offset = 0;
        }
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        self.records
            .record(TransferRecord::Progress { offset, size })
            .await?;
        stream
            .response(
                &DeviceTaskResponse::FileReady {
                    size,
                    offset,
                    sha256: sha256.to_owned(),
                },
                false,
                timeout,
            )
            .await?;

        while offset < size {
            let bytes = stream.receive_binary_frame(timeout).await?;
            let next = offset.saturating_add(bytes.len() as u64);
            if bytes.is_empty() || bytes.len() > MAX_BINARY_FRAME_BYTES || next > size {
                return reject(stream, timeout, "file exceeds declared size").await;
            }
            file.write_all(&bytes).await?;
            offset = next;
            self.records
                .record(TransferRecord::Progress { offset, size })
                .await?;
            stream
                .response(&DeviceTaskResponse::FileProgress { offset }, false, timeout)
                .await?;
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        if hash_file(&partial).await? != sha256 {
            fs::remove_file(&partial).await?;
            return reject(stream, timeout, "file checksum mismatch").await;
        }
        self.records
            .record(TransferRecord::BeginPublication)
            .await?;
        let publication = if overwrite {
            fs::rename(&partial, &destination).await
        } else {
            fs::hard_link(&partial, &destination).await
        };
        if let Err(error) = publication {
            self.records
                .record(TransferRecord::PublicationFailed {
                    message: error.to_string(),
                })
                .await?;
            return Err(error.into());
        }
        // Record the actual publication before sending its receipt. A closed stream
        // after this point must not change an already completed upload into a failure.
        self.records.record(TransferRecord::Completed).await?;
        if !overwrite && let Err(error) = fs::remove_file(&partial).await {
            tracing::warn!(%error, "could not remove published upload staging file");
        }
        stream
            .response(
                &DeviceTaskResponse::FileComplete {
                    size,
                    sha256: sha256.to_owned(),
                },
                true,
                timeout,
            )
            .await?;
        Ok(())
    }

    /// Read-only evidence check, performed with this engine's actual OS identity.
    pub(crate) async fn verify_publication(
        &self,
        path: &str,
        size: u64,
        sha256: &str,
    ) -> Result<bool, TaskServiceError> {
        let Some(path) = valid_path(path) else {
            return Ok(false);
        };
        if !valid_sha256(sha256) {
            return Ok(false);
        }
        let Some(_guard) = self.locks.try_acquire(&path).await? else {
            return Ok(false);
        };
        let meta = fs::symlink_metadata(&path).await?;
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != size {
            return Ok(false);
        }
        Ok(hash_file(&path).await? == sha256)
    }

    pub(crate) async fn download(
        &self,
        stream: &mut impl TransferStream,
        timeout: Duration,
        path: &str,
        offset: u64,
        expected_sha256: Option<&str>,
    ) -> Result<(), TaskServiceError> {
        let Some(source) = valid_path(path) else {
            return reject(stream, timeout, "invalid source path").await;
        };
        if is_symlink(&source).await? {
            return reject(stream, timeout, "source is a symbolic link").await;
        }
        let mut file = match File::open(&source).await {
            Ok(file) => file,
            Err(_) => return reject(stream, timeout, "source file is unavailable").await,
        };
        let metadata = file.metadata().await?;
        if !metadata.is_file() || offset > metadata.len() {
            return reject(
                stream,
                timeout,
                "source is not a regular file or offset is invalid",
            )
            .await;
        }
        let size = metadata.len();
        self.records
            .record(TransferRecord::Progress { offset, size })
            .await?;
        let sha256 = hash_file(&source).await?;
        if expected_sha256.is_some_and(|expected| expected != sha256) {
            return reject(
                stream,
                timeout,
                "source hash changed since the original transfer",
            )
            .await;
        }
        self.records
            .record(TransferRecord::Hash {
                sha256: sha256.clone(),
            })
            .await?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        stream
            .response(
                &DeviceTaskResponse::FileReady {
                    size,
                    offset,
                    sha256: sha256.clone(),
                },
                false,
                timeout,
            )
            .await?;

        let mut sent = offset;
        let mut buffer = vec![0_u8; MAX_BINARY_FRAME_BYTES.min(64 * 1024)];
        while sent < size {
            let limit = usize::try_from((size - sent).min(buffer.len() as u64)).unwrap();
            let read = file.read(&mut buffer[..limit]).await?;
            if read == 0 {
                return reject(stream, timeout, "source changed during transfer").await;
            }
            stream.send_binary_frame(&buffer[..read], timeout).await?;
            sent += read as u64;
            self.records
                .record(TransferRecord::Progress { offset: sent, size })
                .await?;
        }
        self.records.record(TransferRecord::Completed).await?;
        stream
            .response(
                &DeviceTaskResponse::FileComplete { size, sha256 },
                true,
                timeout,
            )
            .await?;
        Ok(())
    }
}

fn valid_path(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute() && path.file_name().is_some() && path.parent()?.is_dir() {
        Some(path.to_owned())
    } else {
        None
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn partial_path(destination: &Path, sha256: &str, namespace: Option<&str>) -> PathBuf {
    let name = destination.file_name().unwrap().to_string_lossy();
    let scope = namespace.map_or_else(String::new, |value| format!("-{}", &value[..16]));
    destination.with_file_name(format!(".{name}.pab-{}{scope}.part", &sha256[..16]))
}

async fn is_symlink(path: &Path) -> Result<bool, std::io::Error> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

async fn hash_file(path: &Path) -> Result<String, std::io::Error> {
    let mut file = File::open(path).await?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; MAX_BINARY_FRAME_BYTES];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

async fn reject(
    stream: &mut impl TransferStream,
    timeout: Duration,
    message: &'static str,
) -> Result<(), TaskServiceError> {
    stream
        .response(
            &DeviceTaskResponse::Error {
                code: DeviceTaskErrorCode::InvalidRequest,
                message: message.to_owned(),
            },
            true,
            timeout,
        )
        .await?;
    Err(TaskServiceError::InvalidRequest(message))
}

#[cfg(test)]
#[path = "transfer_engine_tests.rs"]
mod tests;
