use super::transfer_engine::{LocalRecorder, TransferEngine, TransferStream};
use super::*;
use crate::user_worker::transfer::Operation;
use pab_os_control::execution::PreparedUser;
use pab_protocol::{FileTransferOperation, FileTransferRequest, TransferSnapshot};
use std::io;
use std::path::PathBuf;

impl TaskService {
    pub(super) async fn transfer_stream(
        &self,
        actor: OperatorRef,
        request: &FileTransferRequest,
        stream: &mut PabBiStream,
        timeout: Duration,
    ) -> Result<(), TaskServiceError> {
        request
            .validate()
            .map_err(TaskServiceError::InvalidRequest)?;
        let operational_path = match &request.native_path {
            Some(path) => pab_file_manager::decode_path(path).map_err(|_| {
                TaskServiceError::InvalidRequest("native transfer path is invalid on this platform")
            })?,
            None => PathBuf::from(request.operation.path()),
        };
        if !operational_path.is_absolute() {
            return Err(TaskServiceError::InvalidRequest(
                "transfer path must be absolute",
            ));
        }
        // Observe accepted work before resolving a reference from a new connection.
        if self
            .store
            .existing_transfer(actor, request)
            .await?
            .is_some()
        {
            let snapshot = self.lookup_transfer(actor, request.request_id).await?;
            stream
                .send_json(&DeviceTaskResponse::Transfer { snapshot }, timeout)
                .await?;
            return Ok(());
        }
        let _slot = self
            .filesystem_slots
            .try_acquire()
            .map_err(|_| TaskServiceError::InvalidRequest("executor has 8 active file streams"))?;
        let user = self.prepare_user(actor, request.execution).await?;
        let mut context = self.execution_context.clone();
        if let Some((_, identity)) = &user {
            context.cwd = Some(identity.home.clone());
            context.environment_revision = format!(
                "user-v1:{:x}",
                Sha256::digest(serde_json::to_vec(identity).map_err(TaskStoreError::from)?)
            );
            context.identity = Some(identity.clone());
        }
        if request.expected_execution.as_ref().is_some_and(|expected|!expected.matches_context(&context)) {
            return Err(TaskServiceError::ExecutionContext("file transfer execution identity changed before acceptance".into()));
        }
        // Resolve the worker before accepting work; a missing executable must
        // not leave a durable running operation with nothing to settle it.
        let worker = if user.is_some() {
            Some(self.user_worker_executable()?)
        } else {
            None
        };
        if let Some(original_id) = request.resume_from {
            let original_request = self.store.transfer_request(actor, original_id).await?;
            let original = self.lookup_transfer(actor, original_id).await?;
            let same = match (&original_request.operation, &request.operation) {
                (
                    FileTransferOperation::Upload {
                        path: a,
                        size: b,
                        sha256: c,
                        overwrite: d,
                    },
                    FileTransferOperation::Upload {
                        path: w,
                        size: x,
                        sha256: y,
                        overwrite: z,
                    },
                ) => {
                    a == w
                        && b == x
                        && c == y
                        && d == z
                        && matches!(
                            original.state.as_str(),
                            "failed" | "interrupted" | "cancelled" | "paused"
                        )
                }
                (
                    FileTransferOperation::Download { path: a, .. },
                    FileTransferOperation::Download {
                        path: b,
                        offset,
                        expected_sha256,
                    },
                ) => {
                    a == b
                        && *offset <= original.size
                        && original.sha256.is_some()
                        && expected_sha256 == &original.sha256
                        && matches!(
                            original.state.as_str(),
                            "failed" | "interrupted" | "cancelled" | "paused" | "completed"
                        )
                }
                _ => false,
            };
            let Some(previous) = original.execution_context else {
                return Err(TaskServiceError::ExecutionContext(
                    "original transfer has no recorded execution identity".into(),
                ));
            };
            if !same
                || original_request.native_path != request.native_path
                || original_request.metadata != request.metadata
                || original_request.destination_parent != request.destination_parent
                || original_request.source_constraint != request.source_constraint
                || previous.identity != context.identity
            {
                return Err(TaskStoreError::RequestConflict.into());
            }
            context = previous;
        }
        if !self.store.accept_transfer(actor, request, &context).await? {
            let snapshot = self.lookup_transfer(actor, request.request_id).await?;
            stream
                .send_json(&DeviceTaskResponse::Transfer { snapshot }, timeout)
                .await?;
            return Ok(());
        }
        // Publish the frozen identity before the first binary frame. Failure to
        // deliver this receipt is settled by the same durable transfer record.
        let worker_required=user.is_some();
        let result = async {
            stream
                .send_frame_json(
                    &DeviceTaskResponse::TransferAccepted {
                        snapshot: self.store.get_transfer(actor, request.request_id).await?,
                    },
                    timeout,
                )
                .await?;
            if let Some((prepared, _)) = user {
                let operation = match &request.operation {
                    FileTransferOperation::Upload {
                        path,
                        size,
                        sha256,
                        overwrite,
                    } => Operation::Upload {
                        path: path.clone(),
                        size: *size,
                        sha256: sha256.clone(),
                        overwrite: *overwrite,
                    },
                    FileTransferOperation::Download {
                        path,
                        offset,
                        expected_sha256,
                    } => Operation::Download {
                        path: path.clone(),
                        offset: *offset,
                        expected_sha256: expected_sha256.clone(),
                    },
                };
                let (_send, cancel) = watch::channel(false);
                Box::pin(crate::user_worker::transfer::execute_native(
                    worker
                        .as_ref()
                        .expect("user worker resolved before acceptance"),
                    prepared,
                    operation,
                    request.native_path.clone(),
                    crate::user_worker::transfer::Attributes {destination_parent:request.destination_parent.clone(),metadata:request.metadata.clone(),source_constraint:request.source_constraint.clone()},
                    LocalRecorder {
                        store: self.store.clone(),
                        request_id: request.request_id,
                    },
                    self.file_coordinator(),
                    stream,
                    cancel,
                ))
                .await
                .map_err(TaskServiceError::from)
            } else {
                let engine =
                    TransferEngine::local(&self.store, request.request_id, &self.upload_locks).with_parent(request.destination_parent.clone()).with_metadata(request.metadata.clone()).with_source_constraint(request.source_constraint.clone());
                #[cfg(feature="test-support")]
                let engine=engine.with_barrier(self.transfer_barrier.clone());
                match &request.operation {
                    FileTransferOperation::Upload {
                        path: _,
                        size,
                        sha256,
                        overwrite,
                    } => {
                        engine
                            .upload(
                                stream,
                                timeout,
                                &operational_path,
                                *size,
                                sha256,
                                *overwrite,
                            )
                            .await
                    }
                    FileTransferOperation::Download {
                        path: _,
                        offset,
                        expected_sha256,
                    } => {
                        engine
                            .download(
                                stream,
                                timeout,
                                &operational_path,
                                *offset,
                                expected_sha256.as_deref(),
                            )
                            .await
                    }
                }
            }
        }
        .await;
        if !worker_required && result.is_ok() && self.store.get_transfer(actor,request.request_id).await?.state=="stopping" {
            self.store.finalize_transfer_stopped(request.request_id).await?;
        }
        self.finish_transfer(request.request_id, &result).await?;
        result
    }

    pub(super) async fn lookup_transfer(
        &self,
        actor: OperatorRef,
        id: pab_protocol::RequestId,
    ) -> Result<TransferSnapshot, TaskServiceError> {
        let mut snapshot = self.store.get_transfer(actor, id).await?;
        if snapshot.state != "committing" || snapshot.direction != "receive" {
            return Ok(snapshot);
        }
        // Status polling must share the normal file IO/worker budget rather
        // than start an unbounded set of checksum probes or native processes.
        let Ok(_slot)=self.filesystem_slots.try_acquire() else {return Ok(snapshot);};
        let request=self.store.transfer_request(actor,id).await?;
        let Some(identity) = snapshot
            .execution_context
            .as_ref()
            .and_then(|context| context.identity.as_ref())
            .cloned()
        else {
            return Ok(snapshot);
        };
        let Some(hash) = snapshot.sha256.clone() else {
            return Ok(snapshot);
        };
        if identity.mode == pab_protocol::ExecutionMode::Service {
            // Never probe files under a different service identity after an
            // upgrade/reconfiguration, even if that identity is privileged.
            let same=snapshot.execution_context.as_ref().and_then(|context|pab_protocol::FileExecutionIdentity::from_context(context).ok())
                .is_some_and(|expected|expected.matches_context(&self.execution_context));
            if request.metadata.is_some() && same {
                let path=match &request.native_path {Some(path)=>pab_file_manager::decode_path(path).map_err(|_|TaskServiceError::InvalidRequest("invalid stored native transfer path"))?,None=>PathBuf::from(&snapshot.path)};
                let engine=TransferEngine::local(&self.store,id,&self.upload_locks).with_parent(request.destination_parent).with_metadata(request.metadata);
                if matches!(Box::pin(engine.verify_publication(path,snapshot.size,&hash)).await,Ok(true)) {
                    self.store.finish_transfer(id,"completed",None).await?;
                    snapshot=self.store.get_transfer(actor,id).await?;
                }
            }
            return Ok(snapshot);
        }
        let prepared =
            tokio::task::spawn_blocking(move || PreparedUser::from_observation(&identity)).await?;
        let Ok(prepared) = prepared else {
            return Ok(snapshot);
        };
        let mut evidence = EvidenceStream(false);
        let (_send, cancel) = watch::channel(false);
        let native_path = request.native_path;
        let result = Box::pin(crate::user_worker::transfer::execute_native(
            &self.user_worker_executable()?,
            prepared,
            Operation::Reconcile {
                path: snapshot.path.clone(),
                size: snapshot.size,
                sha256: hash,
            },
            native_path,
            crate::user_worker::transfer::Attributes {destination_parent:request.destination_parent,metadata:request.metadata,source_constraint:None},
            LocalRecorder {
                store: self.store.clone(),
                request_id: id,
            },
            self.file_coordinator(),
            &mut evidence,
            cancel,
        ))
        .await;
        if result.is_ok() && evidence.0 {
            self.store.finish_transfer(id, "completed", None).await?;
            snapshot = self.store.get_transfer(actor, id).await?;
        }
        Ok(snapshot)
    }
}
struct EvidenceStream(bool);
impl TransferStream for EvidenceStream {
    async fn response(
        &mut self,
        value: &DeviceTaskResponse,
        end: bool,
        _: Duration,
    ) -> Result<(), TaskServiceError> {
        if end && matches!(value, DeviceTaskResponse::FileComplete { .. }) {
            self.0 = true;
            Ok(())
        } else {
            Err(io::Error::other("unexpected transfer evidence response").into())
        }
    }
    async fn receive_binary_frame(&mut self, _: Duration) -> Result<Vec<u8>, TaskServiceError> {
        Err(io::Error::other("evidence cannot read network bytes").into())
    }
    async fn send_binary_frame(&mut self, _: &[u8], _: Duration) -> Result<(), TaskServiceError> {
        Err(io::Error::other("evidence cannot write network bytes").into())
    }
}
