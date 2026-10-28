use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use pab_protocol::{DeviceTaskErrorCode, DeviceTaskResponse, RequestId};
use pab_transport::{MAX_BINARY_FRAME_BYTES, PabBiStream};
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, File},
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
    fn end(&mut self,_timeout:Duration)->impl Future<Output=Result<(),TaskServiceError>>+Send {
        async {Err(io::Error::other("stream end is unsupported").into())}
    }
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
    async fn end(&mut self,timeout:Duration)->Result<(),TaskServiceError> {Ok(self.finish_send(timeout).await?)}
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
    CheckStop,
    CancelRequested,
    StopCheckpoint {offset:u64,size:u64},
    StopCompleted,
    Progress { offset: u64, size: u64 },
    Hash { sha256: String },
    BeginPublication,
    Published,
    Metadata {result:pab_protocol::FileTransferMetadataResult},
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
    fn record(&self, event: TransferRecord) -> impl Future<Output=io::Result<()>>+Send {
        // This recorder is used throughout both stream engines. Keep its SQL
        // state on the heap rather than multiplying large query futures inside
        // every frame/stop/hash branch on a normal 2 MiB Windows thread stack.
        Box::pin(async move {
        let id = self.request_id;
        match event {
            TransferRecord::CancelRequested => {
                if self.store.transfer_cancel_pending(id).await.map_err(io::Error::other)? {return Ok(());}
                return Err(io::Error::other("no confirmed cancellation intent"));
            },
            TransferRecord::CheckStop => {
                if self.store.transfer_stop_pending(id).await.map_err(io::Error::other)? {return Err(io::Error::new(io::ErrorKind::Interrupted,"transfer stop requested"));}
                return Ok(());
            },
            TransferRecord::StopCheckpoint {offset,size} => self.store.confirm_transfer_stopped(id,offset,size).await,
            TransferRecord::StopCompleted => self.store.finalize_transfer_stopped(id).await,
            TransferRecord::Progress { offset, size } if offset <= size => {
                self.store.transfer_progress(id, offset, size).await
            }
            TransferRecord::Hash { sha256 } if valid_sha256(&sha256) => {
                self.store.transfer_hash(id, &sha256).await
            }
            TransferRecord::BeginPublication => self.store.begin_transfer_publication(id).await,
            TransferRecord::Published => self.store.confirm_transfer_publication(id).await,
            TransferRecord::Metadata {result} => self.store.record_transfer_metadata(id,&result).await,
            TransferRecord::PublicationFailed { message } if message.len() <= 8192 => {
                self.store.publication_failed(id, &message).await
            }
            TransferRecord::Completed => self.store.finish_transfer(id, "completed", None).await,
            _ => return Err(io::Error::other("invalid transfer record")),
        }
        .map_err(io::Error::other)
        })
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
    metadata:Option<pab_protocol::FileTransferMetadata>,
    source_constraint:Option<pab_protocol::FileTransferSourceConstraint>,
    destination_parent:Option<pab_protocol::FileTransferParentConstraint>,
    records: R,
    locks: FileLocks,
    resume_namespace: Option<String>,
    #[cfg(feature="test-support")]
    barrier:Option<crate::transfer_test_hooks::UploadBarrier>,
}
impl TransferEngine<LocalRecorder> {
    pub(super) fn local(store: &TaskStore, id: RequestId, locks: &UploadPathLocks) -> Self {
        Self {
            metadata:None,
            source_constraint:None,
            destination_parent:None,
            records: LocalRecorder {
                store: store.clone(),
                request_id: id,
            },
            locks: FileLocks::Store(locks.clone()),
            resume_namespace: None,
            #[cfg(feature="test-support")]
            barrier:None,
        }
    }
}
impl<R: TransferRecorder> TransferEngine<R> {
    pub(crate) fn with_parent(mut self,parent:Option<pab_protocol::FileTransferParentConstraint>)->Self {self.destination_parent=parent;self}
    pub(crate) fn with_metadata(mut self,metadata:Option<pab_protocol::FileTransferMetadata>)->Self {self.metadata=metadata;self}
    pub(crate) fn with_source_constraint(mut self,source:Option<pab_protocol::FileTransferSourceConstraint>)->Self {self.source_constraint=source;self}
    #[cfg(feature="test-support")]
    pub(crate) fn with_barrier(mut self,barrier:Option<crate::transfer_test_hooks::UploadBarrier>)->Self {self.barrier=barrier;self}
    async fn io_or_stop<T>(&self,io:impl Future<Output=Result<T,TaskServiceError>>)->Result<Option<T>,TaskServiceError> {
        tokio::pin!(io);
        let mut tick=tokio::time::interval_at(tokio::time::Instant::now()+Duration::from_millis(250),Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {tokio::select! {biased;
            _=tick.tick()=>if self.records.record(TransferRecord::CheckStop).await.is_err(){return Ok(None)},
            result=&mut io=>return result.map(Some),
        }}
    }
    async fn stopped(&self,stream:&mut impl TransferStream,timeout:Duration,mut file:Option<&mut File>,offset:u64,size:u64)->Result<bool,TaskServiceError> {
        self.stopped_with_target(stream,timeout,file.take(),offset,size,None).await
    }
    async fn stopped_upload(&self,stream:&mut impl TransferStream,timeout:Duration,file:&mut File,offset:u64,size:u64,target:&pab_file_manager::StagingTarget)->Result<bool,TaskServiceError> {
        self.stopped_with_target(stream,timeout,Some(file),offset,size,Some(target)).await
    }
    async fn stopped_with_target(&self,stream:&mut impl TransferStream,timeout:Duration,mut file:Option<&mut File>,offset:u64,size:u64,target:Option<&pab_file_manager::StagingTarget>)->Result<bool,TaskServiceError> {
        if self.records.record(TransferRecord::CheckStop).await.is_ok() {return Ok(false);}
        if let Some(file)=file.as_mut() {file.flush().await?;file.sync_all().await?;}
        self.records.record(TransferRecord::StopCheckpoint {offset,size}).await?;
        if let (Some(target),Some(file))=(target,file.as_mut()) {
            // Only explicit cancellation removes the original receiving file.
            // A pause or a failed intent lookup keeps it available for resume.
            if self.records.record(TransferRecord::CancelRequested).await.is_ok() {
                let original=file.try_clone().await?.into_std().await;
                target.remove_open(&original)?;
            }
        }
        // EOF cannot be confused with file bytes by a download receiver.
        stream.end(timeout).await?;
        Ok(true)
    }
    async fn controlled_hash(&self,file:&mut File)->Result<Option<String>,TaskServiceError> {
        file.seek(std::io::SeekFrom::Start(0)).await?;let mut hash=Sha256::new();let mut buffer=vec![0u8;64*1024];
        let mut check=tokio::time::Instant::now();
        loop {
            if tokio::time::Instant::now()>=check {
                if self.records.record(TransferRecord::CheckStop).await.is_err() {return Ok(None);}
                check=tokio::time::Instant::now()+Duration::from_millis(250);
            }
            let read=file.read(&mut buffer).await?;if read==0 {break};hash.update(&buffer[..read]);
        }
        Ok(Some(format!("{:x}",hash.finalize())))
    }
    pub(crate) fn worker(
        records: R,
        locks: mpsc::Sender<FileCoordination>,
        identity: &[u8],
    ) -> Self {
        Self {
            metadata:None,
            source_constraint:None,
            destination_parent:None,
            records,
            locks: FileLocks::Worker(locks),
            resume_namespace: Some(format!("{:x}", Sha256::digest(identity))),
            #[cfg(feature="test-support")]
            barrier:None,
        }
    }
    pub(crate) async fn upload(
        &self,
        stream: &mut impl TransferStream,
        timeout: Duration,
        path: impl AsRef<Path>,
        size: u64,
        sha256: &str,
        overwrite: bool,
    ) -> Result<(), TaskServiceError> {
        if let Some(metadata)=&self.metadata {metadata.validate().map_err(TaskServiceError::InvalidRequest)?;}
        if let Some(source)=&self.source_constraint {
            source.validate().map_err(TaskServiceError::InvalidRequest)?;
            if source.size.0!=size {return Err(TaskServiceError::InvalidRequest("upload size differs from source precondition"));}
        }
        let Some(destination) = valid_path(path.as_ref()) else {
            return reject(stream, timeout, "invalid destination path").await;
        };
        if !valid_sha256(sha256) {
            return reject(stream, timeout, "invalid checksum").await;
        }
        let Some(_upload_guard) = self.locks.try_acquire(&destination).await? else {
            return reject(stream, timeout, "destination already has an active upload").await;
        };
        if let Some(parent)=&self.destination_parent {parent.validate().map_err(TaskServiceError::InvalidRequest)?;}
        let parent=self.destination_parent.clone();
        let partial = partial_path(&destination, sha256, self.resume_namespace.as_deref());
        let (target,file)=tokio::task::spawn_blocking(move || {
            let target=pab_file_manager::StagingTarget::new_in_parent(&partial,&destination,parent.as_ref().map(|value|value.identity.as_str()))?;
            target.check_destination(overwrite)?;let file=target.open(true)?;Ok::<_,std::io::Error>((target,file))
        }).await??;
        let mut file=File::from_std(file);
        let mut offset = file.metadata().await?.len();
        if offset > size {
            file.set_len(0).await?;
            offset = 0;
        }
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        self.records
            .record(TransferRecord::Progress { offset, size })
            .await?;
        if self.stopped_upload(stream,timeout,&mut file,offset,size,&target).await? {return Ok(());}
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

        let mut checkpoint=tokio::time::Instant::now()+Duration::from_millis(250);
        let received=async {
        while offset < size {
            let bytes=match self.io_or_stop(stream.receive_binary_frame(timeout)).await {
                Ok(Some(bytes))=>bytes,
                Ok(None)=>{self.stopped_upload(stream,timeout,&mut file,offset,size,&target).await?;return Ok(false);},
                Err(error)=>{if self.stopped_upload(stream,timeout,&mut file,offset,size,&target).await? {return Ok(false);}return Err(error);}
            };
            let next = offset.saturating_add(bytes.len() as u64);
            if bytes.is_empty() || bytes.len() > MAX_BINARY_FRAME_BYTES || next > size {
                return reject(stream, timeout, "file exceeds declared size").await.map(|()|false);
            }
            file.write_all(&bytes).await?;
            offset = next;
            #[cfg(feature="test-support")]
            if let Some(barrier)=&self.barrier && barrier.visit(path.as_ref(),offset).await && self.stopped_upload(stream,timeout,&mut file,offset,size,&target).await? {return Ok(false);}
            if tokio::time::Instant::now()>=checkpoint {
                if self.stopped_upload(stream,timeout,&mut file,offset,size,&target).await? {return Ok(false);}
                self.records.record(TransferRecord::Progress {offset,size}).await?;
                checkpoint=tokio::time::Instant::now()+Duration::from_millis(250);
            }
            stream
                .response(&DeviceTaskResponse::FileProgress { offset }, false, timeout)
                .await?;
        }
        Ok::<bool,TaskServiceError>(true)
        }.await;
        match received {
            Ok(false)=>return Ok(()),
            Ok(true)=>{},
            Err(error)=>{
                // Only complete frames survive a broken transport/IPC reply.
                // Persist that receiver fact before the parent closes its worker.
                file.set_len(offset).await?;file.flush().await?;file.sync_all().await?;
                self.records.record(TransferRecord::Progress {offset,size}).await?;
                return Err(error);
            },
        }
        file.flush().await?;
        file.sync_all().await?;
        self.records.record(TransferRecord::Progress {offset,size}).await?;
        let Some(digest)=self.controlled_hash(&mut file).await? else {self.stopped_upload(stream,timeout,&mut file,offset,size,&target).await?;return Ok(());};
        if digest != sha256 {
            let file=file.into_std().await;
            tokio::task::spawn_blocking(move ||target.remove_open(&file)).await??;
            return reject(stream, timeout, "file checksum mismatch").await;
        }
        if self.stopped_upload(stream,timeout,&mut file,offset,size,&target).await? {return Ok(());}
        self.records
            .record(TransferRecord::BeginPublication)
            .await?;
        let file=file.into_std().await;
        let metadata=self.metadata.clone();
        // Retain coordination through blocking publication even if its async
        // owner is dropped; another writer must not overtake the OS operation.
        let (publication, _target, _file, _upload_guard) = tokio::task::spawn_blocking(move || {
            let result=(|| {
                if let Some(metadata)=metadata {
                    let before=pab_protocol::FileTransferMetadata {modified_at_unix_ms:metadata.modified_at_unix_ms,unix_mode:None};
                    pab_file_manager::apply_file_metadata(&file,&before)?;
                    file.sync_all()?;
                }
                target.publish_open(&file,overwrite)
            })();
            (result, target, file, _upload_guard)
        })
        .await?;
        if let Err(error) = publication {
            self.records
                .record(TransferRecord::PublicationFailed {
                    message: error.to_string(),
                })
                .await?;
            return Err(error.into());
        }
        // Keep the actual locked file through this durable fact, and eventually
        // through attribute finalization. Intent alone is not a successful rename.
        self.records.record(TransferRecord::Published).await?;
        let (_target,_file)=if let Some(metadata)=self.metadata.clone() {
            let (target,file,result)=tokio::task::spawn_blocking(move || {
                let result=pab_file_manager::apply_file_metadata(&_file,&metadata).and_then(|result|_file.sync_all().map(|()|result));
                (_target,_file,result)
            }).await?;
            self.records.record(TransferRecord::Metadata {result:result?}).await?;
            (target,file)
        } else {(_target,_file)};
        // Release the native byte-range lock before announcing completion so
        // readers may open the final file as soon as they receive its receipt.
        drop(_file);
        drop(_target);
        // A closed stream after this point cannot turn publication into a failure.
        self.records.record(TransferRecord::Completed).await?;
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
        path: impl AsRef<Path>,
        size: u64,
        sha256: &str,
    ) -> Result<bool, TaskServiceError> {
        let Some(path) = valid_path(path.as_ref()) else {
            return Ok(false);
        };
        if !valid_sha256(sha256) {
            return Ok(false);
        }
        let Some(_guard) = self.locks.try_acquire(&path).await? else {
            return Ok(false);
        };
        let parent=self.destination_parent.clone();
        let opened=tokio::task::spawn_blocking(move || {
            let file=pab_file_manager::open_source_in_parent(&path,parent.as_ref().map(|value|value.identity.as_str()))?;
            file.try_lock().map_err(io::Error::other)?;
            Ok::<_,io::Error>(file)
        }).await?;
        let file=match opened {Ok(file)=>file,Err(error) if error.kind()==io::ErrorKind::NotFound=>return Ok(false),Err(error)=>return Err(error.into())};
        let mut file=File::from_std(file);
        let before=file.metadata().await?;
        if before.len() != size {
            return Ok(false);
        }
        if hash_file(&mut file).await? != sha256 {return Ok(false);}
        let after=file.metadata().await?;
        pab_file_manager::verify_opened_source_metadata(&after,size,pab_file_manager::modified_at_unix_ms(&before))?;
        if let Some(requested)=&self.metadata {
            // Recovery observes attributes, never reapplies chmod/set_times.
            // Persist both facts while retaining the independently verified FD.
            let result=pab_file_manager::observe_file_metadata(&after,requested)?;
            self.records.record(TransferRecord::Published).await?;
            self.records.record(TransferRecord::Metadata {result}).await?;
        }
        drop(file.into_std().await);Ok(true)
    }

    pub(crate) async fn download(
        &self,
        stream: &mut impl TransferStream,
        timeout: Duration,
        path: impl AsRef<Path>,
        offset: u64,
        expected_sha256: Option<&str>,
    ) -> Result<(), TaskServiceError> {
        if self.destination_parent.is_some(){return Err(TaskServiceError::InvalidRequest("remote source cannot use receiver parent constraint"));}
        if let Some(source)=&self.source_constraint {source.validate().map_err(TaskServiceError::InvalidRequest)?;}
        let Some(source) = valid_path(path.as_ref()) else {
            return reject(stream, timeout, "invalid source path").await;
        };
        if is_symlink(&source).await? {
            return reject(stream, timeout, "source is a symbolic link").await;
        }
        let mut file = match tokio::task::spawn_blocking(move ||pab_file_manager::open_source(&source)).await? {
            Ok(file) => File::from_std(file),
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
        let modified=pab_file_manager::modified_at_unix_ms(&metadata);
        if let Some(source)=&self.source_constraint {
            pab_file_manager::verify_opened_source_metadata(&metadata,source.size.0,source.modified_at_unix_ms)?;
        }
        self.records
            .record(TransferRecord::Progress { offset, size })
            .await?;
        let Some(sha256)=self.controlled_hash(&mut file).await? else {self.stopped(stream,timeout,None,offset,size).await?;return Ok(());};
        pab_file_manager::verify_opened_source_metadata(&file.metadata().await?,size,modified)?;
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
        let mut checkpoint=tokio::time::Instant::now()+Duration::from_millis(250);
        let mut buffer = vec![0_u8; MAX_BINARY_FRAME_BYTES.min(64 * 1024)];
        while sent < size {
            let limit = usize::try_from((size - sent).min(buffer.len() as u64)).unwrap();
            let read = file.read(&mut buffer[..limit]).await?;
            if read == 0 {
                return reject(stream, timeout, "source changed during transfer").await;
            }
            match self.io_or_stop(stream.send_binary_frame(&buffer[..read],timeout)).await {
                Ok(Some(()))=>{},
                Ok(None)=>{self.stopped(stream,timeout,None,sent,size).await?;return Ok(());},
                Err(error)=>{if self.stopped(stream,timeout,None,sent,size).await? {return Ok(());}return Err(error);}
            }
            sent += read as u64;
            if tokio::time::Instant::now()>=checkpoint {
                if self.stopped(stream,timeout,None,sent,size).await? {return Ok(());}
                self.records.record(TransferRecord::Progress {offset:sent,size}).await?;
                checkpoint=tokio::time::Instant::now()+Duration::from_millis(250);
            }
        }
        self.records.record(TransferRecord::Progress {offset:sent,size}).await?;
        pab_file_manager::verify_opened_source_metadata(&file.metadata().await?,size,modified)?;
        if self.stopped(stream,timeout,None,sent,size).await? {return Ok(());}
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

fn valid_path(path: &Path) -> Option<PathBuf> {
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
    let scope = namespace.map_or_else(String::new, |value| format!("-{}", &value[..16]));
    pab_file_manager::publish::staging_path(destination, &format!("pab-{}{scope}", &sha256[..16]))
        .expect("validated filename and digest marker")
}

async fn is_symlink(path: &Path) -> Result<bool, std::io::Error> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

async fn hash_file(file: &mut File) -> Result<String, std::io::Error> {
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
