//! Observe an original directory publication under its frozen native identity.
//! No mutation, target inspection, new admission or service-account fallback.
#[cfg(any(unix, test))]
use super::TaskService;
#[cfg(unix)]
use super::TaskServiceError;
use super::{filesystem::FileError, filesystem_engine::FileEngine};
use pab_protocol::{
    FileManagerOperation, FileManagerRequest, FileManagerResult, FileSystemAction, FileSystemReply,
    FileSystemRequest,
};
#[cfg(unix)]
use pab_protocol::{FileManagerReply, OperatorRef};
use std::io;
#[cfg(any(unix, test))]
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
#[cfg(any(unix, test))]
use tokio::sync::{Mutex, OwnedMutexGuard};
#[cfg(any(unix, test))]
pub(super) type RecoveryGates = Arc<Mutex<HashMap<pab_protocol::RequestId, Arc<Mutex<Instant>>>>>;

pub(crate) fn validate_request(
    request: &FileSystemRequest,
    original: &FileSystemReply,
) -> io::Result<()> {
    request.validate().map_err(io::Error::other)?;
    let FileSystemAction::Managed {
        operation:
            operation @ FileManagerOperation::PrepareDirectory {
                existing_object: None,
                ..
            },
        expected_execution,
    } = &request.operation
    else {
        return Err(io::Error::other("not an original directory creation"));
    };
    let expected = FileSystemReply::pending(request);
    let context = original
        .execution_context
        .as_ref()
        .ok_or_else(|| io::Error::other("missing original directory identity"))?;
    if original.request_id != expected.request_id
        || original.path != expected.path
        || original.kind != expected.kind
        || original.destination != expected.destination
        || !matches!(original.state.as_str(), "unconfirmed" | "completed")
        || original.data_size != 0
        || original.data_sha256.is_some()
        || expected_execution
            .as_ref()
            .is_some_and(|identity| !identity.matches_context(context))
    {
        return Err(io::Error::other(
            "invalid original directory reconciliation",
        ));
    }
    if original.state == "completed" {
        let managed = original
            .file_manager
            .as_ref()
            .ok_or_else(|| io::Error::other("missing original directory fact"))?;
        if original.error.is_some()
            || managed.execution_context != original.execution_context
            || !matches!(
                managed.result,
                FileManagerResult::DirectoryPrepared { created: true, .. }
            )
        {
            return Err(io::Error::other("invalid original directory fact"));
        }
        managed
            .validate_for(&FileManagerRequest {
                schema_version: pab_protocol::FILE_MANAGER_SCHEMA_VERSION,
                request_id: request.request_id,
                execution: request.execution,
                expected_execution: expected_execution.clone(),
                operation: operation.clone(),
            })
            .map_err(io::Error::other)?;
    }
    Ok(())
}
impl FileEngine {
    pub(crate) async fn reconcile_directory(
        &self,
        request: &FileSystemRequest,
        original: FileSystemReply,
    ) -> Result<FileSystemReply, FileError> {
        validate_request(request, &original).map_err(|error| {
            FileError::new("invalid_directory_recovery", "observe", error.to_string())
        })?;
        #[cfg(unix)]
        {
            let context = self.file_manager.as_ref().ok_or_else(|| {
                FileError::new(
                    "identity_unavailable",
                    "observe",
                    "native file manager context is unavailable",
                )
            })?;
            let database = context.cache.clone();
            let id = request.request_id;
            let observation = tokio::task::spawn_blocking(move || {
                pab_file_manager::observe_directory_creation(&database, id)
            })
            .await
            .map_err(|error| FileError::new("worker_unavailable", "observe", error.to_string()))?
            .map_err(|error| FileError::new("receipt_unavailable", "observe", error.to_string()))?;
            match observation {
                pab_file_manager::DirectoryCreationObservation::Absent
                    if original.state == "completed" =>
                {
                    self.store
                        .directory_receipt(&original, true)
                        .await
                        .map_err(|error| {
                            FileError::new(
                                "outcome_unconfirmed",
                                "record_release",
                                error.to_string(),
                            )
                        })?;
                }
                pab_file_manager::DirectoryCreationObservation::Recorded(receipt) => {
                    let FileSystemAction::Managed { operation, .. } = &request.operation else {
                        unreachable!()
                    };
                    if &receipt.fact().operation != operation
                        || receipt.fact().request_id != request.request_id
                    {
                        return Err(FileError::new(
                            "receipt_conflict",
                            "observe",
                            "directory receipt differs from the original operation",
                        ));
                    }
                    let mut result = original;
                    result.state = "completed".into();
                    result.error = None;
                    result.file_manager = Some(FileManagerReply {
                        schema_version: pab_protocol::FILE_MANAGER_SCHEMA_VERSION,
                        request_id: request.request_id,
                        execution_context: result.execution_context.clone(),
                        result: receipt.fact().result.clone(),
                    });
                    self.store
                        .directory_receipt(&result, false)
                        .await
                        .map_err(|error| {
                            FileError::new(
                                "outcome_unconfirmed",
                                "record_directory",
                                error.to_string(),
                            )
                        })?;
                    if let Err(error) = receipt.acknowledge() {
                        tracing::warn!(%error,"recovered directory result saved; original receipt remains");
                    } else if let Err(error) = self.store.directory_receipt(&result, true).await {
                        tracing::warn!(%error,"directory release notification requires reconciliation");
                    }
                    return Ok(result);
                }
                _ => {}
            }
        }
        Ok(original)
    }
}
#[cfg(any(unix, test))]
impl TaskService {
    pub(super) async fn directory_recovery_gate(
        &self,
        id: pab_protocol::RequestId,
    ) -> Option<OwnedMutexGuard<Instant>> {
        let mut gates = self.directory_recovery_gates.lock().await;
        gates.retain(|_, gate| {
            Arc::strong_count(gate) > 1
                || gate
                    .try_lock()
                    .is_ok_and(|last| last.elapsed() < Duration::from_secs(60))
        });
        let gate = if let Some(gate) = gates.get(&id) {
            gate.clone()
        } else {
            if gates.len() >= 256 {
                return None;
            }
            let now = Instant::now();
            let gate = Arc::new(Mutex::new(
                now.checked_sub(Duration::from_secs(2)).unwrap_or(now),
            ));
            gates.insert(id, gate.clone());
            gate
        };
        drop(gates);
        let mut guard = gate.try_lock_owned().ok()?;
        if guard.elapsed() < Duration::from_secs(2) {
            return None;
        }
        *guard = Instant::now();
        Some(guard)
    }
    #[cfg(unix)]
    pub(super) async fn reconcile_directory_query(
        &self,
        actor: OperatorRef,
        original: FileSystemReply,
    ) -> Result<FileSystemReply, TaskServiceError> {
        let Some(execution) = self
            .store
            .filesystem_execution(actor, original.request_id)
            .await?
        else {
            return Ok(original);
        };
        if !execution.creation_receipt_pending {
            return Ok(original);
        }
        let Some(context) = execution.context else {
            return Ok(original);
        };
        if original.execution_context.as_ref() != Some(&context)
            || validate_request(&execution.request, &original).is_err()
        {
            return Ok(original);
        }
        let Some(identity) = context.identity.clone() else {
            return Ok(original);
        };
        let Some(mut gate) = self.directory_recovery_gate(original.request_id).await else {
            return Ok(original);
        };
        if identity.mode == pab_protocol::ExecutionMode::User {
            let prepared = tokio::task::spawn_blocking(move || {
                pab_os_control::execution::PreparedUser::from_observation(&identity)
            })
            .await;
            if let Ok(Ok(prepared)) = prepared {
                if let Err(error) = crate::user_worker::filesystem::reconcile_directory(
                    &self.user_worker_executable()?,
                    prepared,
                    execution.request,
                    original.clone(),
                    self.file_coordinator(),
                    self.file_manager_scope(actor)?,
                )
                .await
                {
                    tracing::warn!(id=%original.request_id,%error,"native directory recovery remains pending");
                }
            }
        } else if identity.mode == pab_protocol::ExecutionMode::Service
            && pab_protocol::FileExecutionIdentity::from_context(&context)
                .is_ok_and(|identity| identity.matches_context(&self.execution_context))
        {
            if let Err(error) = self
                .managed_engine(actor)?
                .reconcile_directory(&execution.request, original.clone())
                .await
            {
                tracing::warn!(id=%original.request_id,%error,"service directory recovery remains pending");
            }
        }
        *gate = Instant::now();
        self.store
            .get_filesystem(actor, original.request_id)
            .await
            .map_err(Into::into)
    }
}
