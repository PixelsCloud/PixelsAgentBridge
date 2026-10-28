use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use pab_protocol::{
    DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, DeviceTaskResponse,
    FileTransferOperation, FileTransferOptions, FileTransferRequest,
    RequestId,
};
use pab_transport::{MAX_BINARY_FRAME_BYTES, PabBiStream};
use sha2::{Digest, Sha256};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

use super::{AuthenticatedDeviceConnection, BridgeError, TransferControl};

// Shared by Desktop and MCP, including files that are still being hashed. The
// pipeline budget alone would not bound buffers of streams waiting for credit.
static TRANSFER_IO_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

enum DownloadTarget {
    Legacy(pab_file_manager::StagingTarget),
    #[cfg(unix)]
    Private(pab_file_manager::PrivateFileStaging),
}
impl From<pab_file_manager::StagingTarget> for DownloadTarget {
    fn from(value:pab_file_manager::StagingTarget)->Self {Self::Legacy(value)}
}
impl DownloadTarget {
    fn publish_open(&self,file:&std::fs::File,overwrite:bool)->std::io::Result<pab_file_manager::publish::PublicationReceipt> {
        match self {Self::Legacy(target)=>target.publish_open(file,overwrite),#[cfg(unix)] Self::Private(target)=>target.publish_open(file,overwrite)}
    }
    fn remove_open(&self,file:&std::fs::File)->std::io::Result<()> {
        match self {Self::Legacy(target)=>target.remove_open(file),#[cfg(unix)] Self::Private(target)=>target.discard_open(file)}
    }
}

/// Keep the original receiving handle even when its async operation is dropped.
/// The cancellation/publication gate prevents removal after publication wins.
struct CancelledDownloadCleanup {
    target:std::sync::Arc<DownloadTarget>,
    file:std::fs::File,
    control:TransferControl,
}
impl Drop for CancelledDownloadCleanup {
    fn drop(&mut self) {
        if self.control.discard_cancelled_staging() {
            self.control.record_cancelled_cleanup(self.target.remove_open(&self.file));
            #[cfg(unix)]
            if let DownloadTarget::Private(target)=&*self.target {
                if !target.retire_empty_container().unwrap_or(false) {tracing::debug!("cancelled receiving container retained");}
            }
        }
    }
}

impl AuthenticatedDeviceConnection {
    pub async fn require_managed_transfer(&self)->Result<(),BridgeError> {
        match self.task_request(DeviceTaskRequest::GetEnvironment {schema_version:DEVICE_TASK_SCHEMA_VERSION}).await? {
            DeviceTaskResponse::Environment {context,filesystem_schema_version:Some(files),transfer_schema_version:Some(transfer),..} if context.device_ref==self.device_ref && files>=11 && transfer>=pab_protocol::FILE_TRANSFER_PARENT_SCHEMA_VERSION=>Ok(()),
            _=>Err(BridgeError::FileTransfer("Update the target application to enable independent file transfer".into())),
        }
    }
    pub async fn stop_transfer(&self,id:RequestId,intent:pab_protocol::FileTransferIntent)->Result<pab_protocol::TransferSnapshot,BridgeError> {
        if intent==pab_protocol::FileTransferIntent::Run {return Err(BridgeError::FileTransfer("stop requires pause or cancel".into()));}
        match self.task_request(DeviceTaskRequest::GetEnvironment {schema_version:DEVICE_TASK_SCHEMA_VERSION}).await? {
            DeviceTaskResponse::Environment {context,transfer_schema_version:Some(version),..} if context.device_ref==self.device_ref && version>=pab_protocol::FILE_TRANSFER_CONTROL_SCHEMA_VERSION=>{},
            _=>return Err(BridgeError::FileTransfer("Update the target application to support confirmed transfer stops".into())),
        }
        match self.task_request(DeviceTaskRequest::StopTransfer {schema_version:DEVICE_TASK_SCHEMA_VERSION,request_id:id,intent}).await? {
            DeviceTaskResponse::Transfer {snapshot} if snapshot.request_id==id && snapshot.initiated_by==self.operator=>self.get_transfer(id).await,
            other=>Err(unexpected(other)),
        }
    }
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
        self.upload_file_paths(
            request_id,
            source,
            destination,
            None,
            overwrite,
            options,
            control,
            progress,
        )
        .await
    }

    pub async fn upload_native_file_with_options(
        &self,
        request_id: RequestId,
        source: &Path,
        destination: &pab_protocol::NativeFilePath,
        overwrite: bool,
        options: &FileTransferOptions,
        control: &TransferControl,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), BridgeError> {
        destination
            .validate()
            .map_err(|error| BridgeError::FileTransfer(error.into()))?;
        self.upload_file_paths(
            request_id,
            source,
            &destination.display_label(),
            Some(destination.clone()),
            overwrite,
            options,
            control,
            progress,
        )
        .await
    }

    async fn upload_file_paths(
        &self,
        request_id: RequestId,
        source: &Path,
        destination: &str,
        native_path: Option<pab_protocol::NativeFilePath>,
        overwrite: bool,
        options: &FileTransferOptions,
        control: &TransferControl,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), BridgeError> {
        let _slot = TRANSFER_IO_SLOTS
            .acquire()
            .await
            .map_err(|_| BridgeError::FileTransfer("file transfer scheduler closed".into()))?;
        control.check()?;
        let versioned = self
            .transfer_version(options, native_path.is_some())
            .await?;
        let source_path=source.to_owned();
        let mut file=File::from_std(tokio::task::spawn_blocking(move ||pab_file_manager::open_source(&source_path)).await.map_err(|error|BridgeError::FileTransfer(error.to_string()))??);
        let metadata = file.metadata().await?;
        if !metadata.is_file() {
            return Err(BridgeError::FileTransfer(
                "source is not a regular file".to_owned(),
            ));
        }
        let size = metadata.len();
        if let Some(source)=&options.source_constraint {
            pab_file_manager::verify_opened_source_metadata(&metadata,source.size.0,source.modified_at_unix_ms)?;
        }
        if !control.size_matches(size) {return Err(BridgeError::FileTransfer("source size changed after selection".into()));}
        control.check_source_metadata(&metadata)?;
        let sha256 = hash_open_file(&mut file,control).await?;
        let after_hash=file.metadata().await?;
        pab_file_manager::verify_opened_source_metadata(&after_hash,size,pab_file_manager::modified_at_unix_ms(&metadata))?;
        control.check_source_metadata(&after_hash)?;
        control.set_digest(&sha256);
        let timeout = self.operation_timeout().max(Duration::from_secs(60));
        let mut stream = self.connection.open_bi(timeout).await?;
        control.request()?;
        stream
            .send_frame_json(
                &if versioned {
                    DeviceTaskRequest::TransferFile {
                        schema_version: DEVICE_TASK_SCHEMA_VERSION,
                        request: FileTransferRequest { destination_parent:options.destination_parent.clone(), source_constraint:options.source_constraint.clone(),metadata:options.metadata.clone(),expected_execution:options.expected_execution.clone(),
                            native_path,
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
        super::upload_pipeline::upload(
            stream, &mut file, offset, size, &sha256, timeout, control, progress,
        )
        .await?;
        if let Some(metadata)=&options.metadata {
            // FileComplete proves the bytes, but attributes have their own
            // durable outcome. Query the original operation without replaying.
            let snapshot=self.get_transfer(request_id).await?;
            if snapshot.state!="completed" || snapshot.finished_at_unix_ms.is_none()
                || snapshot.published!=Some(true) || snapshot.direction!="receive"
                || snapshot.path!=destination || snapshot.size!=size || snapshot.offset!=size
                || snapshot.sha256.as_deref()!=Some(sha256.as_str()) {
                return Err(BridgeError::FileTransfer("upload publication outcome is not confirmed".into()));
            }
            snapshot.metadata_result.as_ref()
                .ok_or_else(||BridgeError::FileTransfer("upload attributes are not confirmed".into()))?
                .validate_for(metadata).map_err(|error|BridgeError::FileTransfer(error.into()))?;
            control.accept(snapshot)?;
        }
        Ok(())
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
        self.download_file_paths(
            request_id,
            source,
            None,
            destination,
            overwrite,
            options,
            expected_sha256,
            control,
            progress,
        )
        .await
    }

    pub async fn download_native_file_with_options(
        &self,
        request_id: RequestId,
        source: &pab_protocol::NativeFilePath,
        destination: &Path,
        overwrite: bool,
        options: &FileTransferOptions,
        expected_sha256: Option<&str>,
        control: &TransferControl,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), BridgeError> {
        source
            .validate()
            .map_err(|error| BridgeError::FileTransfer(error.into()))?;
        self.download_file_paths(
            request_id,
            &source.display_label(),
            Some(source.clone()),
            destination,
            overwrite,
            options,
            expected_sha256,
            control,
            progress,
        )
        .await
    }

    async fn download_file_paths(
        &self,
        request_id: RequestId,
        source: &str,
        native_path: Option<pab_protocol::NativeFilePath>,
        destination: &Path,
        overwrite: bool,
        options: &FileTransferOptions,
        expected_sha256: Option<&str>,
        control: &TransferControl,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), BridgeError> {
        let _slot = TRANSFER_IO_SLOTS
            .acquire()
            .await
            .map_err(|_| BridgeError::FileTransfer("file transfer scheduler closed".into()))?;
        control.check()?;
        let versioned = self
            .transfer_version(options, native_path.is_some())
            .await?;
        if destination.file_name().is_none() {
            return Err(BridgeError::FileTransfer(
                "destination is invalid".to_owned(),
            ));
        }
        let partial = partial_path(destination, request_id);
        let parent=options.destination_parent.clone();
        let previous=options.resume_from.map(|original|partial_path(destination,original));let destination_path=destination.to_owned();
        #[cfg(unix)]
        let managed=control.has_publication_guard();
        #[cfg(unix)]
        let receipt=control.resume_staging();
        let (target,file)=tokio::task::spawn_blocking(move || {
            #[cfg(unix)]
            if managed {
                let (target,file)=if let Some(receipt)=receipt {
                    if receipt.destination!=pab_file_manager::encode_path(&destination_path)
                        || parent.as_ref().is_some_and(|constraint|constraint.identity!=receipt.parent_identity) {
                        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,"download staging destination changed"));
                    }
                    pab_file_manager::PrivateFileStaging::reopen(&receipt)?
                } else {
                    if previous.is_some(){return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,"download resume has no private staging ownership receipt"));}
                    pab_file_manager::PrivateFileStaging::create(&destination_path,overwrite,parent.as_ref().map(|value|value.identity.as_str()))?
                };
                return Ok((DownloadTarget::Private(target),file));
            }
            let target=pab_file_manager::StagingTarget::new_in_parent(&partial,&destination_path,parent.as_ref().map(|value|value.identity.as_str()))?;target.check_destination(overwrite)?;
            if let Some(previous)=previous {match target.adopt(&previous) {Ok(())=>{},Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{},Err(error)=>return Err(error)}}
            let file=target.open(true)?;Ok::<_,std::io::Error>((DownloadTarget::Legacy(target),file))
        }).await.map_err(|error|BridgeError::FileTransfer(error.to_string()))??;
        let target=std::sync::Arc::new(target);
        let _cancelled_cleanup=CancelledDownloadCleanup {target:target.clone(),file:file.try_clone()?,control:control.clone()};
        #[cfg(unix)]
        if let DownloadTarget::Private(target)=&*target {control.persist_staging(target.receipt().clone()).await?;}
        let mut file=File::from_std(file);
        let offset = file.metadata().await?.len();
        let timeout = self.operation_timeout().max(Duration::from_secs(60));
        let mut stream = self.connection.open_bi(timeout).await?;
        control.request()?;
        stream
            .send_frame_json(
                &if versioned {
                    DeviceTaskRequest::TransferFile {
                        schema_version: DEVICE_TASK_SCHEMA_VERSION,
                        request: FileTransferRequest { destination_parent:None, source_constraint:options.source_constraint.clone(),metadata:options.metadata.clone(),expected_execution:options.expected_execution.clone(),
                            native_path,
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
        let mut received=offset;
        let receive_result=async {
        let ready = response(&mut stream, timeout).await?;
        let (size, sha256) = match ready {
            DeviceTaskResponse::FileReady {
                size,
                offset: accepted,
                sha256,
            } if accepted == offset
                && offset <= size
                && control.size_matches(size)
                && options.source_constraint.as_ref().is_none_or(|source|source.size.0==size)
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
        let mut checkpoint_at=tokio::time::Instant::now()+Duration::from_millis(250);
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
            if tokio::time::Instant::now()>=checkpoint_at {
                file.flush().await?;file.sync_all().await?;control.set_checkpoint(received);
                checkpoint_at=tokio::time::Instant::now()+Duration::from_millis(250);
            }
            progress(received, size);
        }
        match response(&mut stream, timeout).await? {
            DeviceTaskResponse::FileComplete {
                size: completed,
                sha256: digest,
            } if completed == size && digest == sha256 => {}
            other => return Err(unexpected(other)),
        }
        Ok::<_,BridgeError>((size,sha256))
        }.await;
        let (_size,sha256)=match receive_result {
            Ok(result)=>result,
            Err(error)=>{
                // A half-written chunk is never a resumable checkpoint. Only
                // complete received frames survive a stop or transport error.
                file.set_len(received).await?;
                file.flush().await?;file.sync_all().await?;
                control.set_checkpoint(received);
                return Err(error);
            }
        };
        file.flush().await?;
        file.sync_all().await?;
        control.set_checkpoint(received);
        if hash_open_file(&mut file,control).await? != sha256 {
            let file=file.into_std().await;
            if let Err(error) = target.remove_open(&file) {
                tracing::warn!(%error, "could not remove invalid download staging file");
            } else {
                #[cfg(unix)]
                if matches!(&*target,DownloadTarget::Private(_)) {control.persist_staging_discarded().await?;}
            }
            drop(file);
            return Err(BridgeError::FileTransfer(
                "download checksum mismatch".to_owned(),
            ));
        }
        #[cfg(test)]
        control.before_publication().await;
        let file=file.into_std().await;
        publish_download_shared(target,file,overwrite,options.metadata.clone(),control).await
    }

    async fn transfer_version(
        &self,
        options: &FileTransferOptions,
        native: bool,
    ) -> Result<bool, BridgeError> {
        if let Some(parent)=&options.destination_parent {parent.validate().map_err(|error|BridgeError::FileTransfer(error.into()))?;}
        if let Some(metadata)=&options.metadata {metadata.validate().map_err(|error|BridgeError::FileTransfer(error.into()))?;}
        if let Some(source)=&options.source_constraint {source.validate().map_err(|error|BridgeError::FileTransfer(error.into()))?;}
        if !native && (options.destination_parent.is_some() || options.expected_execution.is_some() || options.source_constraint.is_some() || options.metadata.is_some()) {
            return Err(BridgeError::FileTransfer("file identity, attributes and source constraints require native paths".into()));
        }
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
                let required = options.required_schema_version(native);
                let supported = transfer_schema_version.is_some_and(|v| v >= required);
                if !supported
                    && (options.destination_parent.is_some() || options.expected_execution.is_some() || options.source_constraint.is_some() || options.metadata.is_some() || native || !options.execution.is_service() || options.resume_from.is_some())
                {
                    return Err(BridgeError::FileTransfer(format!(
                        "device requires transfer v{required}; update the Executor before transferring with these options"
                    )));
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

/// Retain the verified receiving handle across rename and final permissions.
/// Persist the actual rename before permissions can make reopening impossible.
#[cfg(test)]
async fn publish_download(target:impl Into<DownloadTarget>,file:std::fs::File,overwrite:bool,metadata:Option<pab_protocol::FileTransferMetadata>,control:&TransferControl)->Result<(),BridgeError> {
    publish_download_shared(std::sync::Arc::new(target.into()),file,overwrite,metadata,control).await
}
async fn publish_download_shared(target:std::sync::Arc<DownloadTarget>,file:std::fs::File,overwrite:bool,metadata:Option<pab_protocol::FileTransferMetadata>,control:&TransferControl)->Result<(),BridgeError> {
    control.expect_local_metadata(metadata.clone())?;
    let before=metadata.as_ref().map(|value|pab_protocol::FileTransferMetadata {modified_at_unix_ms:value.modified_at_unix_ms,unix_mode:None});
    let file=tokio::task::spawn_blocking(move || {
        if let Some(before)=before {pab_file_manager::apply_file_metadata(&file,&before)?;file.sync_all()?;}
        Ok::<_,std::io::Error>(file)
    }).await.map_err(|error|BridgeError::FileTransfer(error.to_string()))??;
    control.persist_publication_intent().await?;
    control.commit()?;
    let publisher=control.clone();
    let (target,file)=tokio::task::spawn_blocking(move || {
        let result=target.publish_open(&file,overwrite);publisher.record_local_publication(result.is_ok());result?;
        Ok::<_,std::io::Error>((target,file))
    }).await.map_err(|error|BridgeError::FileTransfer(error.to_string()))??;
    control.persist_local_publication().await?;
    if let Some(metadata)=metadata {
        let recorder=control.clone();
        let (_target,file)=tokio::task::spawn_blocking(move || {
            let result=pab_file_manager::apply_file_metadata(&file,&metadata)?;file.sync_all()?;
            recorder.record_local_metadata(result)?;Ok::<_,BridgeError>((target,file))
        }).await.map_err(|error|BridgeError::FileTransfer(error.to_string()))??;
        control.persist_local_publication().await?;
        drop(file);
    } else {drop(file);}
    Ok(())
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
    pab_file_manager::publish::staging_path(destination, &format!("pab-{request_id}"))
        .expect("validated destination name and UUID marker")
}

/// A disconnected attempt may keep only its last flushed checkpoint. Opening
/// takes the same native file lock as the writer; a still-live writer is never
/// truncated. Absence of staging is not proof of local publication.
pub(crate) async fn restore_download_checkpoint(id:RequestId,destination:&Path,size:u64,checkpoint:Option<u64>,parent:Option<&pab_protocol::FileTransferParentConstraint>)->Result<Option<u64>,BridgeError> {
    restore_download_checkpoint_owned(id,destination,size,checkpoint,parent,None).await
}

pub(crate) async fn restore_download_checkpoint_owned(id:RequestId,destination:&Path,size:u64,checkpoint:Option<u64>,parent:Option<&pab_protocol::FileTransferParentConstraint>,staging:Option<&pab_file_manager::PrivateFileStagingReceipt>)->Result<Option<u64>,BridgeError> {
    let partial=partial_path(destination,id);let destination=destination.to_owned();let parent=parent.cloned();
    let staging=staging.cloned();
    let opened=tokio::task::spawn_blocking(move || {
        if let Some(receipt)=staging {
            if receipt.destination!=pab_file_manager::encode_path(&destination)
                || parent.as_ref().is_some_and(|constraint|constraint.identity!=receipt.parent_identity) {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,"download staging destination changed"));
            }
            #[cfg(unix)]
            return match pab_file_manager::PrivateFileStaging::reopen(&receipt) {
                Ok((target,file))=>Ok(Some((DownloadTarget::Private(target),file))),
                Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Ok(None),
                Err(error)=>Err(error),
            };
            #[cfg(not(unix))]
            return Err(std::io::Error::new(std::io::ErrorKind::Unsupported,"private staging receipt is not native to this platform"));
        }
        let target=pab_file_manager::StagingTarget::new_in_parent(&partial,&destination,parent.as_ref().map(|value|value.identity.as_str()))?;
        match target.open(false){Ok(file)=>Ok(Some((DownloadTarget::Legacy(target),file))),Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Ok(None),Err(error)=>Err(error)}
    }).await.map_err(|error|BridgeError::FileTransfer(error.to_string()))??;
    let Some((_target,file))=opened else {return Ok(None);};let mut file=File::from_std(file);let length=file.metadata().await?.len();let checkpoint=checkpoint.unwrap_or(0);
    if checkpoint>length || length>size{return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,"local staging no longer matches its durable checkpoint").into());}
    if length!=checkpoint{file.set_len(checkpoint).await?;}
    file.flush().await?;file.sync_all().await?;drop(file.into_std().await);Ok(Some(checkpoint))
}

/// Recovery reads the final object through the same no-follow source opener
/// as transfers; observing publication never rewrites or follows a new link.
pub(crate) async fn verify_download_publication(destination:&Path,size:u64,digest:&str,parent:Option<&pab_protocol::FileTransferParentConstraint>)->Result<bool,BridgeError> {
    if !valid_sha256(digest){return Ok(false);}
    let path=destination.to_owned();let parent=parent.cloned();
    let opened=tokio::task::spawn_blocking(move || pab_file_manager::open_source_in_parent(&path,parent.as_ref().map(|value|value.identity.as_str()))).await.map_err(|error|BridgeError::FileTransfer(error.to_string()))?;
    let file=match opened {Ok(file)=>file,Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(false),Err(error)=>return Err(error.into())};
    let mut file=File::from_std(file);if file.metadata().await?.len()!=size{return Ok(false);}
    Ok(hash_open_file(&mut file,&TransferControl::default()).await?==digest)
}

/// Read-only recovery of a missing attribute receipt. Acquiring the final
/// object's native lock proves its original writer has released that handle.
/// The file is never reopened by name between hashing and observing attributes.
pub(crate) async fn recover_download_metadata(destination:&Path,size:u64,digest:&str,requested:&pab_protocol::FileTransferMetadata,parent:Option<&pab_protocol::FileTransferParentConstraint>)->Result<Option<pab_protocol::FileTransferMetadataResult>,BridgeError> {
    requested.validate().map_err(|error|BridgeError::FileTransfer(error.into()))?;
    if !valid_sha256(digest){return Ok(None);}
    let path=destination.to_owned();let parent=parent.cloned();
    let opened=tokio::task::spawn_blocking(move || {
        let file=pab_file_manager::open_source_in_parent(&path,parent.as_ref().map(|value|value.identity.as_str()))?;
        file.try_lock().map_err(std::io::Error::other)?;
        Ok::<_,std::io::Error>(file)
    }).await.map_err(|error|BridgeError::FileTransfer(error.to_string()))?;
    let file=match opened {Ok(file)=>file,Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(None),Err(error)=>return Err(error.into())};
    let mut file=File::from_std(file);let before=file.metadata().await?;
    if before.len()!=size || hash_open_file(&mut file,&TransferControl::default()).await?!=digest {return Ok(None);}
    let after=file.metadata().await?;
    pab_file_manager::verify_opened_source_metadata(&after,size,pab_file_manager::modified_at_unix_ms(&before))?;
    let result=pab_file_manager::observe_file_metadata(&after,requested)?;
    drop(file.into_std().await);Ok(Some(result))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

async fn hash_open_file(file:&mut File,control:&TransferControl)->Result<String,BridgeError> {
    file.seek(std::io::SeekFrom::Start(0)).await?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; MAX_BINARY_FRAME_BYTES];
    loop {
        control.check()?;
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
#[cfg(test)]
use tokio::fs;
