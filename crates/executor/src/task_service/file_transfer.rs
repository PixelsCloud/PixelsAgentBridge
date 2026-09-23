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

use super::{TaskServiceError, send_error};
use crate::task_store::TaskStore;

pub(super) async fn upload(
    store: &TaskStore,
    request_id: RequestId,
    stream: &mut PabBiStream,
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
    let partial = partial_path(&destination, sha256);
    if is_symlink(&partial).await? {
        return reject(stream, timeout, "partial file is a symbolic link").await;
    }
    let mut file = OpenOptions::new()
        .create(true)
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
    store.transfer_progress(request_id, offset, size).await?;
    stream
        .send_frame_json(
            &DeviceTaskResponse::FileReady {
                size,
                offset,
                sha256: sha256.to_owned(),
            },
            timeout,
        )
        .await?;

    while offset < size {
        let bytes = stream.receive_binary_frame(timeout).await?;
        let next = offset.saturating_add(bytes.len() as u64);
        if next > size {
            return reject(stream, timeout, "file exceeds declared size").await;
        }
        file.write_all(&bytes).await?;
        offset = next;
        store.transfer_progress(request_id, offset, size).await?;
        stream
            .send_frame_json(&DeviceTaskResponse::FileProgress { offset }, timeout)
            .await?;
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    if hash_file(&partial).await? != sha256 {
        fs::remove_file(&partial).await?;
        return reject(stream, timeout, "file checksum mismatch").await;
    }
    if overwrite {
        fs::rename(&partial, &destination).await?;
    } else {
        fs::hard_link(&partial, &destination).await?;
        fs::remove_file(&partial).await?;
    }
    stream
        .send_json(
            &DeviceTaskResponse::FileComplete {
                size,
                sha256: sha256.to_owned(),
            },
            timeout,
        )
        .await?;
    Ok(())
}

pub(super) async fn download(
    store: &TaskStore,
    request_id: RequestId,
    stream: &mut PabBiStream,
    timeout: Duration,
    path: &str,
    offset: u64,
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
    store.transfer_progress(request_id, offset, size).await?;
    let sha256 = hash_file(&source).await?;
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    stream
        .send_frame_json(
            &DeviceTaskResponse::FileReady {
                size,
                offset,
                sha256: sha256.clone(),
            },
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
        store.transfer_progress(request_id, sent, size).await?;
    }
    stream
        .send_json(&DeviceTaskResponse::FileComplete { size, sha256 }, timeout)
        .await?;
    Ok(())
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

fn partial_path(destination: &Path, sha256: &str) -> PathBuf {
    let name = destination.file_name().unwrap().to_string_lossy();
    destination.with_file_name(format!(".{name}.pab-{}.part", &sha256[..16]))
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
    stream: &mut PabBiStream,
    timeout: Duration,
    message: &'static str,
) -> Result<(), TaskServiceError> {
    send_error(
        stream,
        timeout,
        DeviceTaskErrorCode::InvalidRequest,
        message,
    )
    .await?;
    Err(TaskServiceError::InvalidRequest(message))
}
