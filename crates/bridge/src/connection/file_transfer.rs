use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use pab_protocol::{DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, DeviceTaskResponse, RequestId};
use pab_transport::{MAX_BINARY_FRAME_BYTES, PabBiStream};
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

use super::{AuthenticatedDeviceConnection, BridgeError};

impl AuthenticatedDeviceConnection {
    pub async fn upload_file(
        &self,
        request_id: RequestId,
        source: &Path,
        destination: &str,
        overwrite: bool,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), BridgeError> {
        let metadata = fs::metadata(source).await?;
        if !metadata.is_file() {
            return Err(BridgeError::FileTransfer(
                "source is not a regular file".to_owned(),
            ));
        }
        let size = metadata.len();
        let sha256 = hash_file(source).await?;
        let mut file = File::open(source).await?;
        let timeout = self.operation_timeout().max(Duration::from_secs(60));
        let mut stream = self.connection.open_bi(timeout).await?;
        stream
            .send_frame_json(
                &DeviceTaskRequest::UploadFile {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    request_id,
                    path: destination.to_owned(),
                    size,
                    sha256: sha256.clone(),
                    overwrite,
                },
                timeout,
            )
            .await?;
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
        let partial = partial_path(destination);
        if is_symlink(&partial).await? {
            return Err(BridgeError::FileTransfer(
                "partial file is a symbolic link".to_owned(),
            ));
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&partial)
            .await?;
        let offset = file.metadata().await?.len();
        let timeout = self.operation_timeout().max(Duration::from_secs(60));
        let mut stream = self.connection.open_bi(timeout).await?;
        stream
            .send_frame_json(
                &DeviceTaskRequest::DownloadFile {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    request_id,
                    path: source.to_owned(),
                    offset,
                    overwrite,
                },
                timeout,
            )
            .await?;
        let ready = response(&mut stream, timeout).await?;
        let (size, sha256) = match ready {
            DeviceTaskResponse::FileReady {
                size,
                offset: accepted,
                sha256,
            } if accepted == offset && offset <= size && valid_sha256(&sha256) => (size, sha256),
            other => return Err(unexpected(other)),
        };
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        progress(offset, size);
        let mut received = offset;
        while received < size {
            let bytes = stream.receive_binary_frame(timeout).await?;
            let next = received.saturating_add(bytes.len() as u64);
            if next > size {
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
            fs::remove_file(&partial).await?;
            return Err(BridgeError::FileTransfer(
                "download checksum mismatch".to_owned(),
            ));
        }
        if overwrite {
            fs::rename(&partial, destination).await?;
        } else {
            fs::hard_link(&partial, destination).await?;
            fs::remove_file(&partial).await?;
        }
        Ok(())
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

fn partial_path(destination: &Path) -> PathBuf {
    let name = destination.file_name().unwrap().to_string_lossy();
    destination.with_file_name(format!(".{name}.pab.part"))
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
