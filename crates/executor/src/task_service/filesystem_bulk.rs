use super::{TaskService, TaskServiceError, filesystem::FileError, filesystem_bulk_io as io};
use pab_protocol::{
    FileHashProgress, FileItemResult, FileMutationSummary, FileSystemAction, FileSystemReply,
    FileSystemRequest, OperatorRef, RequestId,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::task::JoinHandle;

pub(super) struct BulkJob {
    pub cancel: Arc<AtomicBool>,
    _task: JoinHandle<()>,
}
#[cfg(test)]
pub(super) struct BulkTestGate {
    pub phase: &'static str,
    pub started: tokio::sync::Notify,
    pub resume: tokio::sync::Notify,
}

pub(super) struct Work {
    pub service: TaskService,
    pub request: FileSystemRequest,
    pub reply: FileSystemReply,
    pub handle: tokio::runtime::Handle,
    pub cancel: Arc<AtomicBool>,
    deadline: Instant,
    last_record: Instant,
}

impl TaskService {
    pub(super) async fn start_bulk(
        &self,
        request: &FileSystemRequest,
    ) -> Result<FileSystemReply, TaskServiceError> {
        let mut reply = FileSystemReply::pending(request);
        reply.mutation = Some(FileMutationSummary {
            phase: "planning".to_owned(),
            ..Default::default()
        });
        reply.progress = Some(FileHashProgress {
            completed_bytes: 0,
            total_bytes: 0,
            updated_at_unix_ms: now(),
        });
        let permit = match self.bulk_slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                reply.state = "failed".to_owned();
                reply.error = Some(
                    FileError::new(
                        "executor_busy",
                        "accept",
                        "Executor has 4 active bulk filesystem operations",
                    )
                    .0,
                );
                self.store.finish_filesystem(&reply).await?;
                return Ok(reply);
            }
        };
        self.store.update_filesystem_progress(&reply).await?;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = self.clone();
        let id = request.request_id;
        let mut work = Work {
            service: worker.clone(),
            request: request.clone(),
            reply: reply.clone(),
            handle: tokio::runtime::Handle::current(),
            cancel: cancel.clone(),
            deadline: Instant::now() + Duration::from_secs(30 * 60),
            last_record: Instant::now(),
        };
        let mut jobs = self.bulk_jobs.lock().await;
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work.run()));
            match result {
                Ok(Ok(())) => work.reply.state = "completed".to_owned(),
                Ok(Err(error)) => {
                    work.reply.state = if error.0.code == "cancelled" {
                        "cancelled"
                    } else {
                        "failed"
                    }
                    .to_owned();
                    work.reply.error = Some(error.0);
                }
                Err(_) => {
                    work.reply.state = "unconfirmed".to_owned();
                    work.reply.error = Some(FileError::new("worker_panicked", "execute", "worker stopped before its outcome could be confirmed; query the original ID").0);
                }
            }
            let changed = work
                .reply
                .mutation
                .as_ref()
                .is_some_and(|m| m.published_entries > 0 || m.deleted_entries > 0);
            work.reply.mutation.as_mut().unwrap().partial =
                work.reply.state != "completed" && changed;
            if let Err(error) = work
                .handle
                .block_on(worker.store.finish_filesystem(&work.reply))
            {
                tracing::warn!(%id, %error, "failed to persist bulk filesystem result");
            }
            work.handle.block_on(async {
                worker.bulk_jobs.lock().await.remove(&id);
            });
        });
        jobs.insert(
            id,
            BulkJob {
                cancel,
                _task: task,
            },
        );
        Ok(reply)
    }

    pub(super) async fn cancel_bulk(
        &self,
        actor: OperatorRef,
        id: RequestId,
    ) -> Result<FileSystemReply, TaskServiceError> {
        let mut reply = self.store.get_filesystem(actor, id).await?;
        if !matches!(reply.state.as_str(), "running" | "cancel_requested") {
            return Ok(reply);
        }
        let jobs = self.bulk_jobs.lock().await;
        let Some(job) = jobs.get(&id) else {
            return Err(TaskServiceError::InvalidRequest(
                "filesystem worker is not registered; query the original operation",
            ));
        };
        reply.state = "cancel_requested".to_owned();
        self.store.request_filesystem_cancel(&reply).await?;
        job.cancel.store(true, Ordering::Release);
        self.store
            .get_filesystem(actor, id)
            .await
            .map_err(Into::into)
    }
}

impl Work {
    pub fn check(&self) -> Result<(), FileError> {
        if self.cancel.load(Ordering::Acquire) {
            return Err(FileError::new(
                "cancelled",
                "execute",
                "operation stopped after cancellation; published or deleted items are not rolled back",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(FileError::new(
                "time_limit",
                "execute",
                "operation exceeded its 30-minute execution budget",
            ));
        }
        Ok(())
    }

    pub fn phase(&mut self, phase: &str) -> Result<(), FileError> {
        self.reply.mutation.as_mut().unwrap().phase = phase.to_owned();
        self.record(true)
    }
    pub fn record(&mut self, force: bool) -> Result<(), FileError> {
        if force || self.last_record.elapsed() >= Duration::from_millis(250) {
            self.reply.progress.as_mut().unwrap().updated_at_unix_ms = now();
            self.handle
                .block_on(self.service.store.update_filesystem_progress(&self.reply))
                .map_err(|e| FileError::new("record_error", "record", e.to_string()))?;
            self.last_record = Instant::now();
        }
        Ok(())
    }
    pub fn effect(
        &mut self,
        path: &Path,
        action: &str,
        hash: Option<String>,
    ) -> Result<(), FileError> {
        let summary = self.reply.mutation.as_mut().unwrap();
        if action == "deleted" {
            summary.deleted_entries += 1;
        } else {
            summary.published_entries += 1;
        }
        summary.partial = true;
        if !summary.results_truncated {
            summary.results.push(FileItemResult {
                path: path
                    .to_str()
                    .ok_or_else(|| {
                        FileError::new("non_utf8_path", "record", "result path is not UTF-8")
                    })?
                    .to_owned(),
                action: action.to_owned(),
                sha256: hash,
            });
            if summary.results.len() > 64
                || serde_json::to_vec(&summary.results).map_or(true, |b| b.len() > 8 * 1024)
            {
                summary.results.pop();
                summary.results_truncated = true;
            }
        }
        self.record(true)
    }

    #[cfg(test)]
    pub fn gate(&self, phase: &'static str) {
        let gate = self
            .handle
            .block_on(async { self.service.bulk_test_gate.lock().await.clone() });
        if let Some(gate) = gate
            && gate.phase == phase
        {
            gate.started.notify_one();
            self.handle.block_on(gate.resume.notified());
        }
    }
    #[cfg(not(test))]
    pub fn gate(&self, _phase: &'static str) {}

    fn run(&mut self) -> Result<(), FileError> {
        self.check()?;
        let path = std::path::PathBuf::from(&self.request.path);
        match self.request.operation.clone() {
            FileSystemAction::Copy {
                destination,
                recursive,
                overwrite,
                limits,
            } => io::copy_or_move(
                self,
                &path,
                Path::new(&destination),
                recursive,
                overwrite,
                false,
                &limits,
            ),
            FileSystemAction::Move {
                destination,
                recursive,
                overwrite,
                limits,
            } => io::copy_or_move(
                self,
                &path,
                Path::new(&destination),
                recursive,
                overwrite,
                true,
                &limits,
            ),
            FileSystemAction::Delete { recursive, limits } => {
                io::delete(self, &path, recursive, &limits)
            }
            FileSystemAction::ArchiveCreate {
                sources,
                overwrite,
                limits,
            } => super::filesystem_archive::create(self, &sources, &path, overwrite, &limits),
            FileSystemAction::ArchiveExtract {
                destination,
                overwrite,
                max_ratio,
                limits,
            } => super::filesystem_archive::extract(
                self,
                &path,
                Path::new(&destination),
                overwrite,
                max_ratio,
                &limits,
            ),
            _ => unreachable!("bulk worker requires a bulk action"),
        }
    }
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.as_millis().min(i64::MAX as u128) as i64)
}
