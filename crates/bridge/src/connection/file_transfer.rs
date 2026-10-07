use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use pab_protocol::{
    DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, DeviceTaskResponse,
    FILE_TRANSFER_SCHEMA_VERSION, FileTransferOperation, FileTransferOptions, FileTransferRequest,
    RequestId,
};
use pab_transport::{MAX_BINARY_FRAME_BYTES, PabBiStream};
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

use super::{AuthenticatedDeviceConnection, BridgeError, TransferControl};

impl AuthenticatedDeviceConnection {
    pub async fn upload_file(
        &self,
        request_id: RequestId,
        source: &Path,
        destination: &str,
        overwrite: bool,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), BridgeError> {
        self.upload_file_controlled(
            request_id,
            source,
            destination,
            overwrite,
            &TransferControl::default(),
            progress,
        )
        .await
    }

    pub async fn upload_file_controlled(
        &self,
        request_id: RequestId,
        source: &Path,
        destination: &str,
        overwrite: bool,
        control: &TransferControl,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), BridgeError> {
        self.upload_file_with_options(
            request_id,
            source,
            destination,
            overwrite,
            &FileTransferOptions::default(),
            control,
            progress,
        )
        .await
    }

    pub async fn upload_file_with_options(
        &self,
        request_id: RequestId,
        source: &Path,
        destination: &str,
        overwrite: bool,
        options: &FileTransferOptions,
        control: &TransferControl,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), BridgeError> {
        control.check()?;
        let versioned = self.transfer_version(options).await?;
        let metadata = fs::symlink_metadata(source).await?;
        if !metadata.is_file() {
            return Err(BridgeError::FileTransfer(
                "source is not a regular file".to_owned(),
            ));
        }
        let size = metadata.len();
        let sha256 = hash_file(source).await?;
        control.set_digest(&sha256);
        let mut file = File::open(source).await?;
        let timeout = self.operation_timeout().max(Duration::from_secs(60));
        let mut stream = self.connection.open_bi(timeout).await?;
        control.request()?;
        stream
            .send_frame_json(
                &if versioned {
                    DeviceTaskRequest::TransferFile {
                        schema_version: DEVICE_TASK_SCHEMA_VERSION,
                        request: FileTransferRequest {
                            request_id,
                            execution: options.execution,
                            resume_from: options.resume_from,
                            operation: FileTransferOperation::Upload {
                                path: destination.to_owned(),
                                size,
                                sha256: sha256.clone(),
                                overwrite,
                            },
                        },
                    }
                } else {
                    DeviceTaskRequest::UploadFile {
                        schema_version: DEVICE_TASK_SCHEMA_VERSION,
                        request_id,
                        path: destination.to_owned(),
                        size,
                        sha256: sha256.clone(),
                        overwrite,
                    }
                },
                timeout,
            )
            .await?;
        if versioned {
            self.transfer_accepted(
                &mut stream,
                timeout,
                request_id,
                options,
                "receive",
                destination,
                control,
            )
            .await?;
        }
        let ready = response(&mut stream, timeout).await?;
        let offset = match ready {
            DeviceTaskResponse::FileReady {
                size: remote_size,
                offset,
                sha256: remote_hash,
            } if remote_size == size && remote_hash == sha256 && offset <= size => offset,
            other => return Err(unexpected(other)),
        };
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        progress(offset, size);
        let mut sent = offset;
        let mut buffer = vec![0_u8; MAX_BINARY_FRAME_BYTES.min(64 * 1024)];
        while sent < size {
            control.check()?;
            let limit = usize::try_from((size - sent).min(buffer.len() as u64)).unwrap();
            let read = file.read(&mut buffer[..limit]).await?;
            if read == 0 {
                return Err(BridgeError::FileTransfer(
                    "source changed during transfer".to_owned(),
                ));
            }
            stream.send_binary_frame(&buffer[..read], timeout).await?;
            sent += read as u64;
            match response(&mut stream, timeout).await? {
                DeviceTaskResponse::FileProgress { offset } if offset == sent => {
                    progress(sent, size)
                }
                other => return Err(unexpected(other)),
            }
        }
        match response(&mut stream, timeout).await? {
            DeviceTaskResponse::FileComplete {
                size: completed,
                sha256: digest,
            } if completed == size && digest == sha256 => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    pub async fn download_file(
        &self,
        request_id: RequestId,
        source: &str,
        destination: &Path,
        overwrite: bool,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), BridgeError> {
        self.download_file_controlled(
            request_id,
            source,
            destination,
            overwrite,
            &TransferControl::default(),
            progress,
        )
        .await
    }

    pub async fn download_file_controlled(
        &self,
        request_id: RequestId,
        source: &str,
        destination: &Path,
        overwrite: bool,
        control: &TransferControl,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), BridgeError> {
        self.download_file_with_options(
            request_id,
            source,
            destination,
            overwrite,
            &FileTransferOptions::default(),
            None,
            control,
            progress,
        )
        .await
    }

    pub async fn download_file_with_options(
        &self,
        request_id: RequestId,
        source: &str,
        destination: &Path,
        overwrite: bool,
        options: &FileTransferOptions,
        expected_sha256: Option<&str>,
        control: &TransferControl,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), BridgeError> {
        control.check()?;
        let versioned = self.transfer_version(options).await?;
        if destination.file_name().is_none() {
            return Err(BridgeError::FileTransfer(
                "destination is invalid".to_owned(),
            ));
        }
        match fs::symlink_metadata(destination).await {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(BridgeError::FileTransfer(
                    "destination is not a regular file".to_owned(),
                ));
            }
            Ok(_) if !overwrite => {
                return Err(BridgeError::FileTransfer(
                    "destination already exists".to_owned(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let partial = partial_path(destination, request_id);
        if let Some(original) = options.resume_from {
            let previous = partial_path(destination, original);
            match fs::symlink_metadata(&previous).await {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    // Each explicit attempt takes ownership of the partial file;
                    // a later resume can refer to this attempt again.
                    if fs::symlink_metadata(&partial).await.is_ok() {
                        return Err(BridgeError::FileTransfer(
                            "resume staging path already exists".into(),
                        ));
                    }
                    fs::rename(previous, &partial).await?;
                }
                Ok(_) => {
                    return Err(BridgeError::FileTransfer(
                        "resume source is not a regular file".into(),
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        if is_symlink(&partial).await? {
            return Err(BridgeError::FileTransfer(
                "partial file is a symbolic link".to_owned(),
            ));
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&partial)
            .await?;
        let offset = file.metadata().await?.len();
        let timeout = self.operation_timeout().max(Duration::from_secs(60));
        let mut stream = self.connection.open_bi(timeout).await?;
        control.request()?;
        stream
            .send_frame_json(
                &if versioned {
                    DeviceTaskRequest::TransferFile {
                        schema_version: DEVICE_TASK_SCHEMA_VERSION,
                        request: FileTransferRequest {
                            request_id,
                            execution: options.execution,
                            resume_from: options.resume_from,
                            operation: FileTransferOperation::Download {
                                path: source.to_owned(),
                                offset,
                                expected_sha256: expected_sha256.map(str::to_owned),
                            },
                        },
                    }
                } else {
                    DeviceTaskRequest::DownloadFile {
                        schema_version: DEVICE_TASK_SCHEMA_VERSION,
                        request_id,
                        path: source.to_owned(),
                        offset,
                        overwrite,
                    }
                },
                timeout,
            )
            .await?;
        if versioned {
            self.transfer_accepted(
                &mut stream,
                timeout,
                request_id,
                options,
                "send",
                source,
                control,
            )
            .await?;
        }
        let ready = response(&mut stream, timeout).await?;
        let (size, sha256) = match ready {
            DeviceTaskResponse::FileReady {
                size,
                offset: accepted,
                sha256,
            } if accepted == offset
                && offset <= size
                && valid_sha256(&sha256)
                && expected_sha256.is_none_or(|expected| expected == sha256) =>
            {
                (size, sha256)
            }
            other => return Err(unexpected(other)),
        };
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        control.set_digest(&sha256);
        progress(offset, size);
        let mut received = offset;
        while received < size {
            control.check()?;
            let bytes = stream.receive_binary_frame(timeout).await?;
            let next = received.saturating_add(bytes.len() as u64);
            if bytes.is_empty() || next > size {
                return Err(BridgeError::FileTransfer(
                    "remote file exceeds declared size".to_owned(),
                ));
            }
            file.write_all(&bytes).await?;
            received = next;
            progress(received, size);
        }
        match response(&mut stream, timeout).await? {
            DeviceTaskResponse::FileComplete {
                size: completed,
                sha256: digest,
            } if completed == size && digest == sha256 => {}
            other => return Err(unexpected(other)),
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        if hash_file(&partial).await? != sha256 {
            if let Err(error) = fs::remove_file(&partial).await {
                tracing::warn!(%error, "could not remove invalid download staging file");
            }
            return Err(BridgeError::FileTransfer(
                "download checksum mismatch".to_owned(),
            ));
        }
        control.commit()?;
        if overwrite {
            fs::rename(&partial, destination).await?;
        } else {
            fs::hard_link(&partial, destination).await?;
            if let Err(error) = fs::remove_file(&partial).await {
                tracing::warn!(%error, "could not remove published download staging file");
            }
        }
        Ok(())
    }

    async fn transfer_version(&self, options: &FileTransferOptions) -> Result<bool, BridgeError> {
        if !matches!(
            options.execution,
            pab_protocol::ExecutionSelection::Service {}
                | pab_protocol::ExecutionSelection::User { .. }
        ) {
            return Err(BridgeError::FileTransfer(
                "transfer supports service or user execution only".into(),
            ));
        }
        match self
            .task_request(DeviceTaskRequest::GetEnvironment {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
            })
            .await?
        {
            DeviceTaskResponse::Environment {
                context,
                transfer_schema_version,
                ..
            } if context.device_ref == self.device_ref => {
                let supported =
                    transfer_schema_version.is_some_and(|v| v >= FILE_TRANSFER_SCHEMA_VERSION);
                if !supported && (!options.execution.is_service() || options.resume_from.is_some())
                {
                    return Err(BridgeError::FileTransfer("device does not support transfer v2; user execution and explicit resume require an updated Executor".into()));
                }
                Ok(supported)
            }
            other => Err(unexpected(other)),
        }
    }

    async fn transfer_accepted(
        &self,
        stream: &mut PabBiStream,
        timeout: Duration,
        id: RequestId,
        options: &FileTransferOptions,
        direction: &str,
        path: &str,
        control: &TransferControl,
    ) -> Result<(), BridgeError> {
        match stream.receive_json::<DeviceTaskResponse>(timeout).await? {
            DeviceTaskResponse::Error { message, .. } => {
                control.rejected();
                Err(BridgeError::FileTransfer(message))
            }
            DeviceTaskResponse::TransferAccepted { snapshot }
                if snapshot.request_id == id && snapshot.initiated_by == self.operator
                && snapshot.direction == direction && snapshot.path == path && snapshot.state == "running"
                && snapshot.execution_context.as_ref().is_some_and(|context| {
                    if options.execution.is_service() { context.identity.as_ref().is_none_or(|identity| identity.mode == pab_protocol::ExecutionMode::Service && identity.validate().is_ok()) }
                    else { context.identity.as_ref().is_some_and(|identity| identity.mode == pab_protocol::ExecutionMode::User && identity.validate().is_ok()) }
                }) => control.accept(snapshot),
            DeviceTaskResponse::Transfer { .. } => Err(BridgeError::FileTransfer("transfer was already accepted; query the original operation without replaying bytes".into())),
            other => Err(unexpected(other)),
        }
    }
}

async fn response(
    stream: &mut PabBiStream,
    timeout: Duration,
) -> Result<DeviceTaskResponse, BridgeError> {
    let response = stream.receive_json(timeout).await?;
    match response {
        DeviceTaskResponse::Error { message, .. } => Err(BridgeError::FileTransfer(message)),
        other => Ok(other),
    }
}

fn unexpected(response: DeviceTaskResponse) -> BridgeError {
    BridgeError::FileTransfer(format!("unexpected response: {response:?}"))
}

fn partial_path(destination: &Path, request_id: RequestId) -> PathBuf {
    let name = destination.file_name().unwrap().to_string_lossy();
    destination.with_file_name(format!(".{name}.pab-{request_id}.part"))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
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

#[cfg(test)]
#[path = "file_transfer_tests.rs"]
pub(super) mod tests;
