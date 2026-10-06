use super::*;
use pab_os_control::execution::PreparedUser;
use pab_protocol::{ExecutionIdentity, ExecutionSelection};

impl TaskService {
    pub(super) async fn prepare_user(
        &self,
        actor: OperatorRef,
        selection: ExecutionSelection,
    ) -> Result<Option<(PreparedUser, ExecutionIdentity)>, TaskServiceError> {
        if selection.is_service() {
            return Ok(None);
        }
        let caller = pab_task_runtime::ExecutionCaller {
            device: self.device_ref,
            actor,
            connection: self.ui_connection.id,
        };
        let expected = self
            .ui_connection
            .execution_contexts
            .lock()
            .await
            .identity(caller, selection)
            .map_err(|e| TaskServiceError::ExecutionContext(e.to_string()))?
            .clone();
        let selected = expected.clone();
        let prepared =
            tokio::task::spawn_blocking(move || PreparedUser::from_observation(&selected))
                .await?
                .map_err(|_| {
                    TaskServiceError::ExecutionContext(
                "execution account/session unavailable or changed; query execution contexts again"
                    .into(),
            )
                })?;
        self.ui_connection
            .execution_contexts
            .lock()
            .await
            .resolve(caller, selection, &expected)
            .map_err(|e| TaskServiceError::ExecutionContext(e.to_string()))?;
        Ok(Some((prepared, expected)))
    }
    pub(super) fn user_worker_executable(&self) -> Result<std::path::PathBuf, TaskServiceError> {
        let path = std::env::current_exe()?;
        #[cfg(test)]
        let path = self.worker_executable.clone().unwrap_or(path);
        Ok(path)
    }
}
