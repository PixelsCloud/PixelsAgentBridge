use crate::{
    DeviceCode, DeviceRef, ExecutionSelection, FileByteCount, ManagedFileKind, NativeFilePath,
    RequestId,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileTransferDirection {
    Upload,
    Download,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileConflictPolicy {
    #[default]
    Ask,
    Replace,
    Skip,
    KeepBoth,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileBatchSpec {
    pub id: RequestId,
    pub device: DeviceRef,
    pub code: DeviceCode,
    pub direction: FileTransferDirection,
    pub sources: Vec<NativeFilePath>,
    pub destination: NativeFilePath,
    pub execution: ExecutionSelection,
    pub conflict: FileConflictPolicy,
    pub force_relay: bool,
}
impl FileBatchSpec {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.sources.is_empty() || self.sources.len() > 4096 {
            return Err("select 1..4096 transfer sources");
        }
        if !matches!(
            self.execution,
            ExecutionSelection::Service {} | ExecutionSelection::User { .. }
        ) {
            return Err("invalid transfer execution selection");
        }
        self.destination.validate()?;
        for path in &self.sources {
            path.validate()?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileTransferIntent {
    Run,
    Pause,
    Cancel,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileItemState {
    Queued,
    Preparing,
    WaitingConflict,
    WaitingChildren,
    Transferring,
    Verifying,
    Publishing,
    PauseRequested,
    Paused,
    CancelRequested,
    Cancelled,
    Completed,
    Skipped,
    Interrupted,
    Reconciling,
    Unconfirmed,
    Failed,
}
impl FileItemState {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Cancelled | Self::Completed | Self::Skipped | Self::Failed
        )
    }
    pub fn active(self) -> bool {
        matches!(
            self,
            Self::Preparing
                | Self::Transferring
                | Self::Verifying
                | Self::Publishing
                | Self::PauseRequested
                | Self::CancelRequested
                | Self::Reconciling
                | Self::Unconfirmed
        )
    }
    pub fn can_advance_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        if self.terminal() {
            return false;
        }
        match self {
            Self::WaitingChildren => matches!(next, Self::Preparing | Self::Paused | Self::Cancelled | Self::Failed),
            Self::Queued => matches!(
                next,
                Self::Preparing
                    | Self::Paused
                    | Self::Cancelled
                    | Self::Skipped
                    | Self::WaitingConflict
                    | Self::Failed
            ),
            Self::Preparing => matches!(
                next,
                Self::WaitingChildren | Self::WaitingConflict
                    | Self::Skipped
                    | Self::Paused
                    | Self::Cancelled
                    | Self::Transferring
                    | Self::Verifying
                    | Self::PauseRequested
                    | Self::CancelRequested
                    | Self::Interrupted
                    | Self::Reconciling
                    | Self::Failed
            ),
            Self::WaitingConflict => matches!(
                next,
                Self::Queued | Self::Skipped | Self::Paused | Self::Cancelled | Self::Failed
            ),
            Self::Transferring => matches!(
                next,
                Self::Verifying
                    | Self::PauseRequested
                    | Self::CancelRequested
                    | Self::Interrupted
                    | Self::Reconciling
                    | Self::Failed
            ),
            Self::Verifying => matches!(
                next,
                Self::Publishing
                    | Self::PauseRequested
                    | Self::CancelRequested
                    | Self::Reconciling
                    | Self::Failed
            ),
            Self::Publishing => matches!(
                next,
                Self::WaitingChildren | Self::Completed | Self::Reconciling | Self::Unconfirmed | Self::Failed
            ),
            Self::PauseRequested => matches!(
                next,
                Self::Paused
                    | Self::Verifying
                    | Self::Publishing
                    | Self::Completed
                    | Self::Reconciling
                    | Self::Unconfirmed
                    | Self::Failed
            ),
            Self::CancelRequested => matches!(
                next,
                Self::Cancelled
                    | Self::Publishing
                    | Self::Completed
                    | Self::Reconciling
                    | Self::Unconfirmed
                    | Self::Failed
            ),
            Self::Paused => matches!(next, Self::Queued | Self::WaitingChildren | Self::Cancelled | Self::Failed),
            Self::Interrupted => matches!(
                next,
                Self::Reconciling | Self::Paused | Self::Cancelled | Self::Failed
            ),
            Self::Reconciling | Self::Unconfirmed => matches!(
                next,
                Self::WaitingChildren | Self::Reconciling
                    | Self::Unconfirmed
                    | Self::Completed
                    | Self::Interrupted
                    | Self::Paused
                    | Self::Cancelled
                    | Self::Failed
            ),
            Self::Completed | Self::Cancelled | Self::Skipped | Self::Failed => false,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileBatchItemSpec {
    pub id: RequestId,
    pub source: NativeFilePath,
    pub destination: NativeFilePath,
    pub relative_name: String,
    pub kind: ManagedFileKind,
    pub size: FileByteCount,
    pub modified_at_unix_ms: Option<i64>,
    pub unix_mode: Option<u32>,
}
impl FileBatchItemSpec {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.source.validate()?;
        self.destination.validate()?;
        if self.relative_name.len() > 32768 || self.relative_name.contains('\0') {
            return Err("invalid transfer display name");
        }
        if self.unix_mode.is_some_and(|mode| mode & !0o777 != 0) {
            return Err("transfer cannot preserve privileged mode bits");
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBatchItem {
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub metadata_result:Option<crate::FileTransferMetadataResult>,
    pub spec: FileBatchItemSpec,
    pub state: FileItemState,
    pub revision: FileByteCount,
    pub confirmed: FileByteCount,
    pub attempt: Option<RequestId>,
    pub previous_attempt: Option<RequestId>,
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_and_user_intent_are_not_confused_with_terminal_facts() {
        assert!(!FileItemState::Publishing.can_advance_to(FileItemState::Cancelled));
        assert!(!FileItemState::Publishing.can_advance_to(FileItemState::Paused));
        assert!(FileItemState::CancelRequested.can_advance_to(FileItemState::Completed));
        assert!(FileItemState::PauseRequested.can_advance_to(FileItemState::Reconciling));
        assert!(!FileItemState::Unconfirmed.can_advance_to(FileItemState::Queued));
        for terminal in [
            FileItemState::Completed,
            FileItemState::Cancelled,
            FileItemState::Skipped,
            FileItemState::Failed,
        ] {
            assert!(!terminal.can_advance_to(FileItemState::Transferring));
            assert!(terminal.can_advance_to(terminal));
        }
    }
}
