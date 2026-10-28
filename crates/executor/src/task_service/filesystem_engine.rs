//! File mechanics separated from acceptance, network handling and machine DB.
//! The same engine can run locally or in a user worker; coordination stays with
//! the parent and publication waits for its durable acknowledgement.
use super::{
    TaskService,
    upload_lock::{UploadPathGuard, UploadPathLocks},
};
use crate::task_store::{TaskStore, TaskStoreError};
use pab_protocol::{FileSystemAction, FileSystemReply, FileSystemRequest, RequestId};
use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::sync::{mpsc, oneshot, watch};

pub(crate) enum FileCoordination {
    Lock {
        id: RequestId,
        key: PathBuf,
        answer: oneshot::Sender<io::Result<bool>>,
    },
    Unlock {
        id: RequestId,
    },
    Record {
        publication: bool,
        reply: FileSystemReply,
        answer: oneshot::Sender<io::Result<()>>,
    },
    DirectoryMetadata {
        reply: FileSystemReply,
        answer: oneshot::Sender<io::Result<()>>,
    },
    DirectoryReceipt {reply:FileSystemReply,released:bool,answer:oneshot::Sender<io::Result<()>>},
}
#[derive(Clone)]
pub(crate) struct FileEngine {
    pub(crate) file_manager: Option<super::file_manager::ManagedContext>,
    pub(super) store: FileRecords,
    pub(super) upload_locks: FileLocks,
    #[cfg(test)]
    pub(super) hash_test_gate: std::sync::Arc<
        tokio::sync::Mutex<Option<std::sync::Arc<super::filesystem_hash::HashTestGate>>>,
    >,
    #[cfg(test)]
    pub(super) bulk_test_gate: std::sync::Arc<
        tokio::sync::Mutex<Option<std::sync::Arc<super::filesystem_bulk::BulkTestGate>>>,
    >,
}
impl FileEngine {
    pub(crate) async fn reconcile(&self, mut reply: FileSystemReply) -> FileSystemReply {
        use super::filesystem_io::{digest, load};
        if let Some(meta) = &reply.metadata
            && let Some(expected) = &meta.sha256
            && let Ok(Some(_guard)) = self.upload_locks.try_acquire(Path::new(&reply.path)).await
            && let Ok((bytes, _)) = load(Path::new(&reply.path)).await
            && bytes.len() as u64 == meta.size
            && &digest(&bytes) == expected
        {
            reply.state = "completed".into();
            reply.error = None;
        }
        reply
    }
    /// Execute mechanics only. Acceptance, quotas, history and the final record
    /// belong to the parent. Cancellation must settle before it releases locks.
    pub(crate) async fn execute(
        self,
        request: FileSystemRequest,
        payload: Vec<u8>,
        context: Option<pab_protocol::ExecutionContext>,
        mut cancel: watch::Receiver<bool>,
    ) -> (FileSystemReply, Vec<u8>) {
        use super::filesystem::FileError;
        let mut reply = FileSystemReply::pending(&request);
        reply.execution_context = context;
        if let FileSystemAction::Managed { expected_execution: Some(expected), .. } = &request.operation {
            if reply.execution_context.as_ref().is_none_or(|actual|!expected.matches_context(actual)) {
                reply.state="failed".into();
                reply.error=Some(FileError::new("execution_identity_changed","accept","file-manager account/session differs from the pinned identity").0);
                return (reply,Vec::new());
            }
        }
        if request.operation.is_bulk() {
            use std::sync::{
                Arc,
                atomic::{AtomicBool, Ordering},
            };
            reply.mutation = Some(pab_protocol::FileMutationSummary {
                phase: "planning".into(),
                ..Default::default()
            });
            reply.progress = Some(progress());
            let stop = Arc::new(AtomicBool::new(*cancel.borrow()));
            let mut work =
                super::filesystem_bulk::Work::new(self, request, reply.clone(), stop.clone());
            let mut running = tokio::task::spawn_blocking(move || {
                work.finish();
                work.reply
            });
            let result = tokio::select! {
                result = &mut running => result,
                _ = async { let _ = cancel.wait_for(|value| *value).await; } => {
                    stop.store(true, Ordering::Release);
                    running.await
                }
            };
            return (
                result.unwrap_or_else(|_| {
                    reply.state = "unconfirmed".into();
                    reply.error = Some(
                        FileError::new(
                            "worker_panicked",
                            "execute",
                            "file worker stopped before its outcome was confirmed",
                        )
                        .0,
                    );
                    reply
                }),
                Vec::new(),
            );
        }
        let result = if *cancel.borrow() {
            Err(FileError::new(
                "cancelled",
                "execute",
                "file operation cancelled before execution",
            ))
        } else if matches!(request.operation, FileSystemAction::Hash) {
            reply.progress = Some(progress());
            tokio::time::timeout(
                std::time::Duration::from_secs(30 * 60),
                self.hash_file(&mut reply, &mut cancel),
            )
            .await
            .unwrap_or_else(|_| {
                Err(FileError::new(
                    "time_limit",
                    "hash",
                    "hash exceeded 30 minutes",
                ))
            })
            .map(|_| Vec::new())
        } else if let FileSystemAction::Managed { operation, .. } = &request.operation {
            self.manage_files_controlled(&request, operation, &mut reply, cancel.clone())
                .await
        } else {
            self.prepare_filesystem(&request, &payload, &mut reply)
                .await
        };
        match result {
            Ok(bytes) => {
                reply.state = "completed".into();
                (reply, bytes)
            }
            Err(error) => {
                reply.state = if error.0.code == "outcome_unconfirmed" {
                    "unconfirmed"
                } else if error.0.code == "cancelled" {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
                reply.error = Some(error.0);
                (reply, Vec::new())
            }
        }
    }

    pub(crate) fn worker(send: mpsc::Sender<FileCoordination>) -> Self {
        Self {
            file_manager: None,
            store: FileRecords::Worker(send.clone()),
            upload_locks: FileLocks::Worker(send),
            #[cfg(test)]
            hash_test_gate: Default::default(),
            #[cfg(test)]
            bulk_test_gate: Default::default(),
        }
    }
}
fn progress() -> pab_protocol::FileHashProgress {
    pab_protocol::FileHashProgress {
        completed_bytes: 0,
        total_bytes: 0,
        updated_at_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis().min(i64::MAX as u128) as i64),
    }
}

/// The user process cannot open the machine database or own cross-user locks.
pub(crate) struct FileCoordinator {
    store: TaskStore,
    locks: UploadPathLocks,
    held: std::collections::HashMap<RequestId, UploadPathGuard>,
}
impl FileCoordinator {
    pub(crate) async fn directory_receipt(&self,reply:&FileSystemReply,released:bool)->io::Result<()> {
        self.store.directory_creation_receipt(reply,released).await.map_err(io::Error::other)
    }
    pub(crate) async fn directory_metadata(&self, reply:&FileSystemReply) -> io::Result<()> {
        if self.held.is_empty() || reply.state != "completed" || reply.kind != "file_manager"
            || !matches!(reply.file_manager.as_ref().map(|value|&value.result),Some(pab_protocol::FileManagerResult::DirectoryMetadata {..}|pab_protocol::FileManagerResult::DirectoryPrepared {..})) {
            return Err(io::Error::other("invalid directory metadata outcome"));
        }
        self.store.finish_directory_metadata(reply).await.map_err(io::Error::other)
    }
    pub(crate) fn lock(&mut self, id: RequestId, key: PathBuf) -> io::Result<bool> {
        if self.held.contains_key(&id) || self.held.len() >= 128 || key.as_os_str().len() > 32768 {
            return Err(io::Error::other("invalid file lock request"));
        }
        match self.locks.try_acquire_key(key)? {
            Some(guard) => {
                self.held.insert(id, guard);
                Ok(true)
            }
            None => Ok(false),
        }
    }
    pub(crate) fn unlock(&mut self, id: RequestId) -> io::Result<()> {
        self.held
            .remove(&id)
            .map(|_| ())
            .ok_or_else(|| io::Error::other("unknown file lock"))
    }
    pub(crate) async fn record(
        &self,
        publication: bool,
        reply: &FileSystemReply,
    ) -> io::Result<()> {
        if publication {
            if self.held.is_empty()
                || !matches!(
                    reply.kind.as_str(),
                    "file_write" | "file_patch" | "file_manager"
                )
            {
                return Err(io::Error::other("invalid file publication phase"));
            }
            self.store.begin_file_publication(reply).await
        } else {
            self.store.update_filesystem_progress(reply).await
        }
        .map_err(io::Error::other)
    }
}
impl TaskService {
    pub(crate) fn file_coordinator(&self) -> FileCoordinator {
        FileCoordinator {
            store: self.store.clone(),
            locks: self.upload_locks.clone(),
            held: Default::default(),
        }
    }
    pub(super) fn file_engine(&self) -> FileEngine {
        FileEngine {
            file_manager: None,
            store: FileRecords::Store(self.store.clone()),
            upload_locks: FileLocks::Store(self.upload_locks.clone()),
            #[cfg(test)]
            hash_test_gate: self.hash_test_gate.clone(),
            #[cfg(test)]
            bulk_test_gate: self.bulk_test_gate.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) enum FileRecords {
    Store(TaskStore),
    Worker(mpsc::Sender<FileCoordination>),
}
impl FileRecords {
    pub(super) async fn directory_receipt(&self,reply:&FileSystemReply,released:bool)->Result<(),TaskStoreError> {
        match self {
            Self::Store(store)=>store.directory_creation_receipt(reply,released).await,
            Self::Worker(send)=>{
                let (answer,result)=oneshot::channel();
                send.send(FileCoordination::DirectoryReceipt {reply:reply.clone(),released,answer}).await.map_err(|_|unavailable())?;
                result.await.map_err(|_|unavailable())?.map_err(TaskStoreError::Io)
            }
        }
    }
    pub(super) async fn finish_directory_metadata(&self, reply:&FileSystemReply) -> Result<(),TaskStoreError> {
        match self {
            Self::Store(store) => store.finish_directory_metadata(reply).await,
            Self::Worker(send) => {
                let (answer,result) = oneshot::channel();
                send.send(FileCoordination::DirectoryMetadata {reply:reply.clone(),answer}).await.map_err(|_|unavailable())?;
                result.await.map_err(|_|unavailable())?.map_err(TaskStoreError::Io)
            }
        }
    }
    pub(super) async fn update_filesystem_progress(
        &self,
        reply: &FileSystemReply,
    ) -> Result<(), TaskStoreError> {
        self.record(false, reply).await
    }
    pub(super) async fn begin_file_publication(
        &self,
        reply: &FileSystemReply,
    ) -> Result<(), TaskStoreError> {
        self.record(true, reply).await
    }
    async fn record(
        &self,
        publication: bool,
        reply: &FileSystemReply,
    ) -> Result<(), TaskStoreError> {
        match self {
            Self::Store(store) => {
                if publication {
                    store.begin_file_publication(reply).await
                } else {
                    store.update_filesystem_progress(reply).await
                }
            }
            Self::Worker(send) => {
                let (answer, result) = oneshot::channel();
                send.send(FileCoordination::Record {
                    publication,
                    reply: reply.clone(),
                    answer,
                })
                .await
                .map_err(|_| unavailable())?;
                result
                    .await
                    .map_err(|_| unavailable())?
                    .map_err(TaskStoreError::Io)
            }
        }
    }
}
fn unavailable() -> TaskStoreError {
    TaskStoreError::Io(io::Error::other("file coordinator disconnected"))
}
#[derive(Clone)]
pub(super) enum FileLocks {
    Store(UploadPathLocks),
    Worker(mpsc::Sender<FileCoordination>),
}
pub(super) enum FileGuard {
    Local {
        _guard: UploadPathGuard,
    },
    Remote {
        id: RequestId,
        send: mpsc::Sender<FileCoordination>,
    },
}
impl Drop for FileGuard {
    fn drop(&mut self) {
        if let Self::Remote { id, send } = self {
            // If a saturated/closed queue cannot carry release, the parent
            // retains the lock until worker cleanup rather than releasing early.
            let _ = send.try_send(FileCoordination::Unlock { id: *id });
        }
    }
}
impl FileLocks {
    pub(super) async fn try_acquire(&self, path: &Path) -> io::Result<Option<FileGuard>> {
        match self {
            Self::Store(locks) => Ok(locks
                .try_acquire(path)
                .await?
                .map(|guard| FileGuard::Local { _guard: guard })),
            Self::Worker(send) => {
                let key = UploadPathLocks::canonical_key(path).await?;
                let id = RequestId::new();
                let (answer, result) = oneshot::channel();
                send.send(FileCoordination::Lock { id, key, answer })
                    .await
                    .map_err(|_| io::Error::other("file coordinator disconnected"))?;
                let accepted = result
                    .await
                    .map_err(|_| io::Error::other("file lock result unconfirmed"))??;
                Ok(accepted.then(|| FileGuard::Remote {
                    id,
                    send: send.clone(),
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_service::{filesystem_io::digest, transfer_tests::actor};
    use pab_protocol::{FileSystemAction, FileSystemRequest, TextEncoding};

    #[tokio::test]
    async fn directory_outcome_is_persisted_with_the_object_retained_even_if_acknowledgement_is_lost() {
        for kind in 0..3 {
        for lose_ack in [false,true] {
            let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
            let directory=root.join("target");std::fs::create_dir(&directory).unwrap();
            let path=pab_file_manager::encode_path(&directory);
            let identity=pab_file_manager::DirectoryObject::open(&path,false).unwrap().identity().to_owned();
            let operation=if kind==0 {pab_protocol::FileManagerOperation::SetDirectoryMetadata {path,expected_object:identity,metadata:pab_protocol::FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:None}}}else {
                let expected_parent=pab_file_manager::DirectoryObject::open(&pab_file_manager::encode_path(&root),false).unwrap().identity().to_owned();
                if kind==1 {std::fs::remove_dir(&directory).unwrap();}
                pab_protocol::FileManagerOperation::PrepareDirectory {path,expected_parent,existing_object:(kind==2).then_some(identity)}
            };
            let request=FileSystemRequest {request_id:RequestId::new(),execution:Default::default(),path:"file-manager".into(),operation:FileSystemAction::Managed {expected_execution:None,operation},payload_size:0,payload_sha256:None};
            let store=TaskStore::open(&root.join("tasks.sqlite3")).await.unwrap();
            let context=pab_platform::detect_native_execution_context().unwrap();
            store.accept_filesystem_with_context(actor(),&request,&digest(&serde_json::to_vec(&request).unwrap()),Some(&context)).await.unwrap();
            let (send,mut events)=mpsc::channel(8);let mut engine=FileEngine::worker(send);
            engine.file_manager=Some(super::super::file_manager::ManagedContext {owner:"fixture".into(),cache:root.join("index.sqlite3"),index:Default::default()});
            let (_cancel,receive)=watch::channel(false);let original=request.clone();
            let work=tokio::spawn(engine.execute(original,Vec::new(),Some(context),receive));
            let mut coordinator=FileCoordinator {store:store.clone(),locks:Default::default(),held:Default::default()};
            let mut recorded=None;
            while let Some(event)=events.recv().await {
                match event {
                    FileCoordination::Lock {id,key,answer}=>{answer.send(coordinator.lock(id,key)).unwrap();},
                    FileCoordination::Unlock {id}=>coordinator.unlock(id).unwrap(),
                    FileCoordination::Record {publication,reply,answer}=>{assert!(publication);coordinator.record(publication,&reply).await.unwrap();answer.send(Ok(())).unwrap();},
                    FileCoordination::DirectoryMetadata {reply,answer}=>{
                        assert!(coordinator.locks.try_acquire(&directory).await.unwrap().is_none());
                        #[cfg(windows)] assert!(std::fs::rename(&directory,root.join("must-not-move")).is_err(),"directory must stay retained through recording");
                        coordinator.directory_metadata(&reply).await.unwrap();
                        assert_eq!(store.get_filesystem(actor(),request.request_id).await.unwrap(),reply);
                        recorded=Some(reply);
                        if lose_ack {drop(answer);} else {answer.send(Ok(())).unwrap();}
                    },
                    FileCoordination::DirectoryReceipt {reply,released,answer}=>{assert!(released);coordinator.directory_receipt(&reply,released).await.unwrap();answer.send(Ok(())).unwrap();},
                }
            }
            let (outcome,_)=work.await.unwrap();
            assert_eq!(outcome.state,if lose_ack {"unconfirmed"}else{"completed"});
            assert!(coordinator.held.is_empty());
            let recorded=recorded.unwrap();assert_eq!(store.get_filesystem(actor(),request.request_id).await.unwrap(),recorded);
            // A late worker error must not overwrite the committed receipt.
            store.finish_filesystem(&outcome).await.unwrap();
            let reopened=TaskStore::open(&root.join("tasks.sqlite3")).await.unwrap();
            assert_eq!(reopened.get_filesystem(actor(),request.request_id).await.unwrap(),recorded);
            std::fs::rename(&directory,root.join("released")).unwrap();
        }
        }
    }

    #[tokio::test]
    async fn publication_waits_for_parent_persistence_and_peer_loss_keeps_original_file() {
        for disconnect in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let destination = root.path().join("用户 data.txt");
            tokio::fs::write(&destination, b"original").await.unwrap();
            let store = TaskStore::open(&root.path().join("tasks.sqlite3"))
                .await
                .unwrap();
            let payload = b"updated";
            let request = FileSystemRequest {
                execution: Default::default(),
                request_id: RequestId::new(),
                path: destination.to_str().unwrap().into(),
                operation: FileSystemAction::Write {
                    encoding: TextEncoding::Utf8,
                    overwrite: true,
                    expected_hash: Some(digest(b"original")),
                },
                payload_size: payload.len() as u32,
                payload_sha256: Some(digest(payload)),
            };
            store
                .accept_filesystem(actor(), &request, "fixture-fingerprint")
                .await
                .unwrap();
            let (send, mut events) = mpsc::channel(8);
            let engine = FileEngine::worker(send);
            let requested = request.clone();
            let worker = tokio::spawn(async move {
                let mut reply = FileSystemReply::pending(&requested);
                let result = engine
                    .prepare_filesystem(&requested, payload, &mut reply)
                    .await;
                (result, reply)
            });
            let locks = UploadPathLocks::default();
            let mut held = std::collections::HashMap::new();
            let mut recorded = false;
            while let Some(event) = events.recv().await {
                match event {
                    FileCoordination::Lock { id, key, answer } => {
                        let guard = locks.try_acquire_key(key).unwrap().unwrap();
                        held.insert(id, guard);
                        answer.send(Ok(true)).unwrap();
                        assert!(locks.try_acquire(&destination).await.unwrap().is_none());
                    }
                    FileCoordination::Record {
                        publication,
                        reply,
                        answer,
                    } => {
                        assert!(publication);
                        assert_eq!(reply.request_id, request.request_id);
                        assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"original");
                        if disconnect {
                            drop(answer);
                        } else {
                            store.begin_file_publication(&reply).await.unwrap();
                            assert_eq!(
                                store
                                    .get_filesystem(actor(), request.request_id)
                                    .await
                                    .unwrap()
                                    .state,
                                "unconfirmed"
                            );
                            recorded = true;
                            answer.send(Ok(())).unwrap();
                        }
                    }
                    FileCoordination::Unlock { id } => {
                        assert!(held.remove(&id).is_some());
                    }
                    FileCoordination::DirectoryMetadata { .. }|FileCoordination::DirectoryReceipt {..} => panic!("unexpected directory receipt for file publication"),
                }
            }
            let (result, reply) = worker.await.unwrap();
            assert_eq!(result.is_ok(), !disconnect);
            assert_eq!(recorded, !disconnect);
            assert_eq!(
                tokio::fs::read(&destination).await.unwrap(),
                if disconnect {
                    b"original".as_slice()
                } else {
                    payload.as_slice()
                }
            );
            assert!(held.is_empty());
            assert!(locks.try_acquire(&destination).await.unwrap().is_some());
            if !disconnect {
                assert_eq!(reply.metadata.unwrap().sha256, Some(digest(payload)));
            }
            assert!(
                !std::fs::read_dir(root.path())
                    .unwrap()
                    .filter_map(Result::ok)
                    .any(|e| e.file_name().to_string_lossy().starts_with(".pab-text-"))
            );
        }
    }
}
