use pab_protocol::{ExecutionContext, OsFamily, PathStyle, TaskProgress, TransferProgress};

use crate::TaskRuntimeError;

pub fn validate_execution_context(value: &ExecutionContext) -> Result<(), TaskRuntimeError> {
    validate_text("os_name", &value.os_name, 128)?;
    validate_text("os_version", &value.os_version, 128)?;
    validate_text("environment_revision", &value.environment_revision, 128)?;
    let expected_path_style = match value.os_family {
        OsFamily::Windows => PathStyle::Windows,
        OsFamily::Linux | OsFamily::Macos => PathStyle::Posix,
    };
    if value.path_style != expected_path_style {
        return Err(TaskRuntimeError::PathStyleMismatch);
    }
    if let Some(identity) = &value.identity {
        identity
            .validate()
            .map_err(TaskRuntimeError::InvalidExecutionIdentity)?;
    }
    if let Some(interpreter) = &value.interpreter {
        for (name, field, max_chars) in [
            ("interpreter.id", interpreter.id.as_str(), 128),
            ("interpreter.name", interpreter.name.as_str(), 128),
            ("interpreter.version", interpreter.version.as_str(), 128),
            (
                "interpreter.executable_path",
                interpreter.executable_path.as_str(),
                4_096,
            ),
        ] {
            validate_text(name, field, max_chars)?;
        }
    }
    if let Some(cwd) = value.cwd.as_deref() {
        validate_text("cwd", cwd, 4_096)?;
    }
    Ok(())
}

fn validate_text(
    field: &'static str,
    value: &str,
    max_chars: usize,
) -> Result<(), TaskRuntimeError> {
    if value.trim().is_empty() {
        return Err(TaskRuntimeError::EmptyField(field));
    }
    if value.chars().count() > max_chars {
        return Err(TaskRuntimeError::FieldTooLong { field, max_chars });
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
