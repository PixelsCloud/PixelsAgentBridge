use pab_protocol::{ExecutionContext, OsFamily, PathStyle, TaskProgress, TransferProgress};

use crate::TaskRuntimeError;

pub(crate) fn execution_context(value: &ExecutionContext) -> Result<(), TaskRuntimeError> {
    if value.os_name.trim().is_empty() {
        return Err(TaskRuntimeError::EmptyField("os_name"));
    }
    if value.os_version.trim().is_empty() {
        return Err(TaskRuntimeError::EmptyField("os_version"));
    }
    if value.environment_revision.trim().is_empty() {
        return Err(TaskRuntimeError::EmptyField("environment_revision"));
    }
    let expected_path_style = match value.os_family {
        OsFamily::Windows => PathStyle::Windows,
        OsFamily::Linux | OsFamily::Macos => PathStyle::Posix,
    };
    if value.path_style != expected_path_style {
        return Err(TaskRuntimeError::PathStyleMismatch);
    }
    if let Some(interpreter) = &value.interpreter {
        for (name, field) in [
            ("interpreter.id", interpreter.id.as_str()),
            ("interpreter.name", interpreter.name.as_str()),
            ("interpreter.version", interpreter.version.as_str()),
            (
                "interpreter.executable_path",
                interpreter.executable_path.as_str(),
            ),
        ] {
            if field.trim().is_empty() {
                return Err(TaskRuntimeError::EmptyField(name));
            }
        }
    }
    Ok(())
}

pub(crate) fn progress(
    previous: Option<&TaskProgress>,
    next: &TaskProgress,
) -> Result<(), TaskRuntimeError> {
    match (previous, next) {
        (None, TaskProgress::Transfer(next)) => transfer(None, next),
        (Some(TaskProgress::Transfer(previous)), TaskProgress::Transfer(next)) => {
            transfer(Some(previous), next)
        }
    }
}

fn transfer(
    previous: Option<&TransferProgress>,
    next: &TransferProgress,
) -> Result<(), TaskRuntimeError> {
    if next
        .total_bytes
        .is_some_and(|total| next.confirmed_bytes > total)
    {
        return Err(TaskRuntimeError::ProgressExceedsTotal);
    }
    let Some(previous) = previous else {
        return Ok(());
    };
    if previous.direction != next.direction {
        return Err(TaskRuntimeError::TransferDirectionChanged);
    }
    if next.phase < previous.phase {
        return Err(TaskRuntimeError::TransferPhaseRegressed);
    }
    if next.confirmed_bytes < previous.confirmed_bytes {
        return Err(TaskRuntimeError::ProgressRegressed);
    }
    match (previous.total_bytes, next.total_bytes) {
        (Some(previous), Some(next)) if previous != next => {
            Err(TaskRuntimeError::TransferTotalChanged)
        }
        (Some(_), None) => Err(TaskRuntimeError::TransferTotalRemoved),
        _ => Ok(()),
    }
}
