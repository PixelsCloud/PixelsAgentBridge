use super::{TaskService, TaskServiceError, filesystem::FileError};
use pab_os_control::execution::PreparedUser;
use pab_protocol::{ExecutionContext, FileSystemReply, FileSystemRequest, OperatorRef};
use tokio::sync::watch;

impl TaskService {
    pub(super) async fn run_user_filesystem(
        &self,
        actor: OperatorRef,
        request: FileSystemRequest,
        payload: Vec<u8>,
        user: PreparedUser,
        context: ExecutionContext,
        cancel: watch::Receiver<bool>,
    ) -> Result<(FileSystemReply, Vec<u8>), TaskServiceError> {
        let id = request.request_id;
        let result = crate::user_worker::filesystem::execute(
            &self.user_worker_executable()?,
            user,
            request,
            payload,
            context,
            self.file_coordinator(),
            cancel,
        )
        .await;
        match result {
            Ok(result) => Ok(result),
            Err(error) => {
                // Preserve the last durable publication/progress evidence. A
                // broken IPC channel does not establish that no effect happened.
                let mut reply = self.store.get_filesystem(actor, id).await?;
                reply.state = "unconfirmed".into();
                reply.error = Some(FileError::new("worker_unavailable", "observe", format!("user file worker outcome unconfirmed: {error}; query the original request")).0);
                Ok((reply, Vec::new()))
            }
        }
    }

    pub(super) async fn start_user_filesystem(
        &self,
        actor: OperatorRef,
        request: FileSystemRequest,
        user: PreparedUser,
        context: ExecutionContext,
    ) -> Result<FileSystemReply, TaskServiceError> {
        let is_hash = request.operation.kind() == "file_hash";
        let mut reply = FileSystemReply::pending(&request);
        reply.execution_context = Some(context.clone());
        let slots = if is_hash {
            &self.hash_slots
        } else {
            &self.bulk_slots
        };
        let permit = match slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                reply.state = "failed".into();
                reply.error = Some(
                    FileError::new(
                        "executor_busy",
                        "accept",
                        "Executor has 4 active operations in this filesystem queue",
                    )
                    .0,
                );
                self.store.finish_filesystem(&reply).await?;
                return Ok(reply);
            }
        };
        let (cancel, receive) = watch::channel(false);
        let worker = self.clone();
        let id = request.request_id;
        // Hold the relevant registry through spawn+insert, as existing workers do.
        let mut hash_jobs = if is_hash {
            Some(self.hash_jobs.lock().await)
        } else {
            None
        };
        let mut bulk_jobs = if is_hash {
            None
        } else {
            Some(self.bulk_jobs.lock().await)
        };
        let task = tokio::spawn(async move {
            let _permit = permit;
            match worker
                .run_user_filesystem(actor, request, Vec::new(), user, context, receive)
                .await
            {
                Ok((reply, _)) => {
                    if let Err(error) = worker.store.finish_filesystem(&reply).await {
                        tracing::warn!(%id, %error, "failed to persist user file result");
                    }
                }
                Err(error) => tracing::warn!(%id, %error, "failed to observe user file result"),
            }
            if is_hash {
                worker.hash_jobs.lock().await.remove(&id);
            } else {
                worker.bulk_jobs.lock().await.remove(&id);
            }
        });
        if let Some(jobs) = &mut hash_jobs {
            jobs.insert(
                id,
                super::filesystem_hash::HashJob {
                    cancel,
                    _task: task,
                },
            );
        } else if let Some(jobs) = &mut bulk_jobs {
            jobs.insert(
                id,
                super::filesystem_bulk::BulkJob {
                    cancel: Default::default(),
                    user_cancel: Some(cancel),
                    _task: task,
                },
            );
        }
        Ok(reply)
    }
}
