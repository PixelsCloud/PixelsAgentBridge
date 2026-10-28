use super::{TaskService, filesystem::FileError, filesystem_engine::FileEngine};
use pab_protocol::{
    FileManagerOperation, FileManagerReply, FileManagerResult, FileSystemReply, FileSystemRequest,
    OperatorRef,
};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{OnceCell, watch};

#[derive(Clone)]
pub(crate) struct ManagedContext {
    pub owner: String,
    pub cache: PathBuf,
    pub index: Arc<OnceCell<pab_file_manager::DirectoryIndex>>,
}
impl ManagedContext {
    async fn directory_index(&self)->Result<&pab_file_manager::DirectoryIndex,FileError> {
        self.index.get_or_try_init(||async {
            pab_agent_core::ensure_data_parent(&self.cache).map_err(pab_file_manager::FileManagerError::Io)?;
            #[cfg(unix)]
            {
                let cache=self.cache.clone();
                let report=tokio::task::spawn_blocking(move ||pab_file_manager::recover_directory_staging(&cache)).await.map_err(|error|pab_file_manager::FileManagerError::Io(std::io::Error::other(error)))??;
                if report.retained>0 {tracing::warn!(retained=report.retained,"inactive directory staging receipts retained for diagnosis");}
            }
            pab_file_manager::DirectoryIndex::open(&self.cache).await
        }).await.map_err(failure)
    }
}
impl TaskService {
    pub(super) async fn start_manager_scan(
        &self,
        actor: OperatorRef,
        request: FileSystemRequest,
        context: pab_protocol::ExecutionContext,
    ) -> Result<FileSystemReply, super::TaskServiceError> {
        let mut accepted = FileSystemReply::pending(&request);
        accepted.execution_context = Some(context.clone());
        let permit = match self.bulk_slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                accepted.state = "failed".into();
                accepted.error = Some(
                    FileError::new("executor_busy", "accept", "directory worker limit reached").0,
                );
                self.store.finish_filesystem(&accepted).await?;
                return Ok(accepted);
            }
        };
        self.store.update_filesystem_progress(&accepted).await?;
        let engine = self.managed_engine(actor)?;
        let worker = self.clone();
        let id = request.request_id;
        let (cancel, receive) = watch::channel(false);
        let mut jobs = self.bulk_jobs.lock().await;
        let task = tokio::spawn(async move {
            let _permit = permit;
            let (reply, _) = engine
                .execute(request, Vec::new(), Some(context), receive)
                .await;
            if let Err(error) = worker.store.finish_filesystem(&reply).await {
                tracing::warn!(%id, %error, "directory scan outcome persistence failed");
            }
            worker.bulk_jobs.lock().await.remove(&id);
        });
        jobs.insert(
            id,
            super::filesystem_bulk::BulkJob {
                cancel: Default::default(),
                user_cancel: Some(cancel),
                _task: task,
            },
        );
        Ok(accepted)
    }

    pub(super) fn file_manager_scope(
        &self,
        actor: OperatorRef,
    ) -> Result<String, super::TaskServiceError> {
        let data = serde_json::to_vec(&(actor, self.ui_connection.id))
            .map_err(crate::task_store::TaskStoreError::from)?;
        Ok(super::filesystem_io::digest(&data))
    }
    pub(super) fn managed_engine(
        &self,
        actor: OperatorRef,
    ) -> Result<FileEngine, super::TaskServiceError> {
        let mut engine = self.file_engine();
        engine.file_manager = Some(ManagedContext {
            owner: self.file_manager_scope(actor)?,
            cache: self.file_manager_cache.clone(),
            index: self.file_manager_index.clone(),
        });
        Ok(engine)
    }
}
impl FileEngine {
    pub(super) async fn manage_files(
        &self,
        request: &FileSystemRequest,
        operation: &FileManagerOperation,
        reply: &mut FileSystemReply,
    ) -> Result<Vec<u8>, FileError> {
        let (_stop, cancel) = watch::channel(false);
        self.manage_files_controlled(request, operation, reply, cancel)
            .await
    }

    pub(super) async fn manage_files_controlled(
        &self,
        request: &FileSystemRequest,
        operation: &FileManagerOperation,
        reply: &mut FileSystemReply,
        cancel: watch::Receiver<bool>,
    ) -> Result<Vec<u8>, FileError> {
        let context = self.file_manager.as_ref().ok_or_else(|| {
            FileError::new(
                "access_denied",
                "authorize",
                "file manager requires a verified caller scope",
            )
        })?;
        let result = match operation {
            FileManagerOperation::InspectPathIdentity {path} => {
                let path=path.clone();
                tokio::task::spawn_blocking(move || Ok::<_,pab_file_manager::FileManagerError>(FileManagerResult::PathIdentity {identity:pab_file_manager::inspect_path_identity(&path)?,path})).await.map_err(join_failure)?.map_err(failure)?
            },
            FileManagerOperation::InspectDirectoryObject {path} => {
                let path = path.clone();
                tokio::task::spawn_blocking(move || {
                    let object = pab_file_manager::DirectoryObject::open(&path,false)?;
                    Ok::<_,pab_file_manager::FileManagerError>(FileManagerResult::DirectoryObject {
                        path, identity:object.identity().into(), metadata:object.metadata()?,
                    })
                }).await.map_err(join_failure)?.map_err(failure)?
            },
            FileManagerOperation::PrepareDirectory {path,expected_parent,existing_object} => {
                let native=pab_file_manager::decode_path(path).map_err(failure)?;
                let guard=self.manager_lock(&native).await?;
                self.store.begin_file_publication(reply).await.map_err(record_failure)?;
                let (path,parent,existing)=(path.clone(),expected_parent.clone(),existing_object.clone());
                #[cfg(unix)]
                let (database,id)=(context.cache.clone(),request.request_id);
                let (result,mut object,guard)=tokio::task::spawn_blocking(move || {
                    #[cfg(unix)]
                    let object={
                        pab_agent_core::ensure_data_parent(&database).map_err(pab_file_manager::FileManagerError::Io)?;
                        pab_file_manager::DirectoryObject::prepare_journaled(&database,id,&path,&parent,existing.as_deref())?
                    };
                    #[cfg(not(unix))]
                    let object=pab_file_manager::DirectoryObject::prepare(&path,&parent,existing.as_deref())?;
                    Ok::<_,pab_file_manager::FileManagerError>((FileManagerResult::DirectoryPrepared {path,identity:object.identity().into(),created:existing.is_none()},object,guard))
                }).await.map_err(join_failure)?.map_err(failure)?;
                reply.file_manager=Some(FileManagerReply {schema_version:pab_protocol::FILE_MANAGER_SCHEMA_VERSION,request_id:request.request_id,execution_context:reply.execution_context.clone(),result});
                reply.state="completed".into();
                self.store.finish_directory_metadata(reply).await.map_err(|error|FileError::new("outcome_unconfirmed","record_directory",error.to_string()))?;
                if let Err(error)=object.acknowledge_recorded(){tracing::warn!(%error,"recorded directory creation receipt retained for later acknowledgement");}
                else if cfg!(unix) && existing_object.is_none() {
                    if let Err(error)=self.store.directory_receipt(reply,true).await{tracing::warn!(%error,"directory receipt release notification requires reconciliation");}
                }
                drop((object,guard));return Ok(Vec::new());
            },
            FileManagerOperation::SetDirectoryMetadata {path,expected_object,metadata} => {
                let native = pab_file_manager::decode_path(path).map_err(failure)?;
                let guard = self.manager_lock(&native).await?;
                // Persist intent before the first attribute change. The native
                // worker waits for the parent's durable acknowledgement.
                self.store.begin_file_publication(reply).await.map_err(record_failure)?;
                let (path,expected,metadata) = (path.clone(),expected_object.clone(),metadata.clone());
                let (result,object,guard) = tokio::task::spawn_blocking(move || {
                    let object = pab_file_manager::DirectoryObject::open(&path,true)?;
                    let metadata = object.apply(&expected,&metadata)?;
                    Ok::<_,pab_file_manager::FileManagerError>((FileManagerResult::DirectoryMetadata {path,identity:expected,metadata},object,guard))
                }).await.map_err(join_failure)?.map_err(failure)?;
                reply.file_manager = Some(FileManagerReply {
                    schema_version:pab_protocol::FILE_MANAGER_SCHEMA_VERSION,
                    request_id:request.request_id,execution_context:reply.execution_context.clone(),result,
                });
                reply.state = "completed".into();
                self.store.finish_directory_metadata(reply).await.map_err(|error|FileError::new("outcome_unconfirmed","record_attributes",error.to_string()))?;
                drop((object,guard));
                return Ok(Vec::new());
            },
            FileManagerOperation::Locations {} => FileManagerResult::Locations {
                locations: tokio::task::spawn_blocking(pab_file_manager::locations)
                    .await
                    .map_err(join_failure)?
                    .map_err(failure)?,
            },
            FileManagerOperation::OpenDirectory { path } => {
                let snapshot_id = context.directory_index().await?
                    .scan(&context.owner, path, cancel)
                    .await
                    .map_err(failure)?;
                FileManagerResult::Opened { snapshot_id }
            }
            FileManagerOperation::ReadDirectory {
                snapshot_id,
                after,
                limit,
                filter,
            } => {
                // Managed v7 replies use a bounded 60 KiB envelope, including
                // both copies of the verified execution context.
                let mut limit = *limit;
                loop {
                    let page = context.directory_index().await?
                        .read(
                            &context.owner,
                            *snapshot_id,
                            after.as_deref(),
                            limit,
                            filter,
                        )
                        .await
                        .map_err(failure)?;
                    let result = FileManagerResult::Directory { page };
                    let managed = FileManagerReply {
                        schema_version: pab_protocol::FILE_MANAGER_SCHEMA_VERSION,
                        request_id: request.request_id,
                        execution_context: reply.execution_context.clone(),
                        result,
                    };
                    reply.file_manager = Some(managed);
                    if serde_json::to_vec(reply).is_ok_and(|bytes| bytes.len() <= 59 * 1024) {
                        return Ok(Vec::new());
                    }
                    reply.file_manager = None;
                    if limit <= 1 {
                        return Err(FileError::new(
                            "directory_page_too_large",
                            "browse",
                            "native directory entry exceeds the transport budget",
                        ));
                    }
                    limit = (limit / 2).max(1);
                }
            }
            FileManagerOperation::CloseDirectory { snapshot_id } => {
                context.directory_index().await?
                    .close(&context.owner, *snapshot_id)
                    .await
                    .map_err(failure)?;
                FileManagerResult::Closed {
                    snapshot_id: *snapshot_id,
                }
            }
            FileManagerOperation::Inspect { path } => FileManagerResult::Entry {
                entry: pab_file_manager::inspect(path).await.map_err(failure)?,
            },
            FileManagerOperation::CreateDirectory { parent, name } => {
                let component = pab_file_manager::decode_component(name).map_err(failure)?;
                let parent_path = pab_file_manager::decode_path(parent).map_err(failure)?;
                let guard = self.manager_lock(&parent_path.join(component)).await?;
                self.store
                    .begin_file_publication(reply)
                    .await
                    .map_err(record_failure)?;
                let parent = parent.clone();
                let name = name.clone();
                let path = tokio::task::spawn_blocking(move || {
                    let _guard = guard;
                    pab_file_manager::create_directory_native(&parent, &name)
                })
                .await
                .map_err(join_failure)?
                .map_err(failure)?;
                FileManagerResult::Applied { path }
            }
            FileManagerOperation::Rename { path, name } => {
                let component = pab_file_manager::decode_component(name).map_err(failure)?;
                let native = pab_file_manager::decode_path(path).map_err(failure)?;
                let parent = native.parent().ok_or_else(|| {
                    FileError::new("invalid_path", "rename", "cannot rename a filesystem root")
                })?;
                let source_guard = self.manager_lock(&native).await?;
                let destination = parent.join(component);
                let source_key = super::upload_lock::UploadPathLocks::canonical_key(&native)
                    .await
                    .map_err(|error| {
                        FileError::new("path_unavailable", "coordinate", error.to_string())
                    })?;
                let target_key = super::upload_lock::UploadPathLocks::canonical_key(&destination)
                    .await
                    .map_err(|error| {
                        FileError::new("path_unavailable", "coordinate", error.to_string())
                    })?;
                // Windows case-only renames own the same normalized key. A
                // second acquire would incorrectly conflict with ourselves.
                let target_guard = if source_key == target_key {
                    None
                } else {
                    Some(self.manager_lock(&destination).await?)
                };
                self.store
                    .begin_file_publication(reply)
                    .await
                    .map_err(record_failure)?;
                let path = path.clone();
                let name = name.clone();
                let path = tokio::task::spawn_blocking(move || {
                    let _guards = (source_guard, target_guard);
                    pab_file_manager::rename_native(&path, &name)
                })
                .await
                .map_err(join_failure)?
                .map_err(failure)?;
                FileManagerResult::Applied { path }
            }
            FileManagerOperation::Delete { path, recursive } => {
                let native = pab_file_manager::decode_path(path).map_err(failure)?;
                let guard = self.manager_lock(&native).await?;
                self.store
                    .begin_file_publication(reply)
                    .await
                    .map_err(record_failure)?;
                let path = path.clone();
                let deleted = path.clone();
                let recursive = *recursive;
                tokio::task::spawn_blocking(move || {
                    let _guard = guard;
                    pab_file_manager::delete(&path, recursive)
                })
                .await
                .map_err(join_failure)?
                .map_err(failure)?;
                FileManagerResult::Applied { path: deleted }
            }
        };
        reply.file_manager = Some(FileManagerReply {
            schema_version: pab_protocol::FILE_MANAGER_SCHEMA_VERSION,
            request_id: request.request_id,
            execution_context: reply.execution_context.clone(),
            result,
        });
        Ok(Vec::new())
    }

    async fn manager_lock(
        &self,
        path: &std::path::Path,
    ) -> Result<super::filesystem_engine::FileGuard, FileError> {
        self.upload_locks
            .try_acquire(path)
            .await
            .map_err(|error| FileError::new("path_unavailable", "coordinate", error.to_string()))?
            .ok_or_else(|| {
                FileError::new(
                    "path_busy",
                    "coordinate",
                    "another operation owns this path",
                )
            })
    }
}

fn join_failure(error: tokio::task::JoinError) -> FileError {
    FileError::new("worker_unavailable", "execute", error.to_string())
}
fn record_failure(error: crate::task_store::TaskStoreError) -> FileError {
    FileError::new(
        "persistence_unavailable",
        "before_mutation",
        error.to_string(),
    )
}
fn failure(error: pab_file_manager::FileManagerError) -> FileError {
    use pab_file_manager::FileManagerError as E;
    let code = match &error {
        E::Invalid(_) => "invalid_path",
        E::SnapshotUnavailable => "directory_snapshot_unavailable",
        E::Capacity => "resource_limit",
        E::Interrupted => "cancelled",
        E::Conflict => "request_conflict",
        E::NotFound => "not_found",
        E::OutcomeUnconfirmed(_) => "outcome_unconfirmed",
        E::Io(error) => match error.kind() {
            std::io::ErrorKind::AlreadyExists => "already_exists",
            std::io::ErrorKind::NotFound => "not_found",
            std::io::ErrorKind::PermissionDenied => "access_denied",
            std::io::ErrorKind::DirectoryNotEmpty => "directory_not_empty",
            _ => "filesystem_io",
        },
        E::Database(_) | E::Encoding(_) => "file_manager_storage",
    };
    FileError::new(code, "file_manager", error.to_string())
}
