use pab_protocol::{
    CapabilityRef, ExecutionContext, OperatorRef, OutputAvailability, OutputRange, OutputStream,
    RequestId, TASK_SCHEMA_VERSION, TaskEvent, TaskEventKind, TaskRef, TaskSnapshot, TaskState,
};
use thiserror::Error;

use crate::validation;

#[derive(Debug, Clone)]
pub struct AcceptedTask {
    pub task_ref: TaskRef,
    pub request_id: RequestId,
    pub initiated_by: OperatorRef,
    pub capability: CapabilityRef,
    pub display_summary: String,
    pub execution_context: ExecutionContext,
    pub accepted_at_unix_ms: i64,
}

#[derive(Debug, Clone)]
pub struct TaskAggregate {
    snapshot: TaskSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventApply {
    Applied,
    Duplicate,
}

impl TaskAggregate {
    pub fn accept(value: AcceptedTask) -> Result<(Self, TaskEvent), TaskRuntimeError> {
        if value.capability.name.trim().is_empty() {
            return Err(TaskRuntimeError::EmptyField("capability.name"));
        }
        if value.capability.version == 0 {
            return Err(TaskRuntimeError::ZeroCapabilityVersion);
        }
        if value.display_summary.trim().is_empty() {
            return Err(TaskRuntimeError::EmptyField("display_summary"));
        }
        validation::validate_execution_context(&value.execution_context)?;
        let event = TaskEvent {
            schema_version: TASK_SCHEMA_VERSION,
            task_ref: value.task_ref,
            seq: 1,
            occurred_at_unix_ms: value.accepted_at_unix_ms,
            kind: TaskEventKind::Accepted,
        };
        let snapshot = TaskSnapshot {
            schema_version: TASK_SCHEMA_VERSION,
            task_ref: value.task_ref,
            request_id: value.request_id,
            initiated_by: value.initiated_by,
            capability: value.capability,
            display_summary: value.display_summary,
            state: TaskState::Accepted,
            stage: None,
            latest_event_seq: event.seq,
            created_at_unix_ms: value.accepted_at_unix_ms,
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
            progress: None,
            completion: None,
            error: None,
            output: OutputAvailability::default(),
            execution_context: value.execution_context,
        };
        Ok((Self { snapshot }, event))
    }

    pub fn restore(snapshot: TaskSnapshot) -> Result<Self, TaskRuntimeError> {
        if snapshot.schema_version != TASK_SCHEMA_VERSION {
            return Err(TaskRuntimeError::UnsupportedSchemaVersion(
                snapshot.schema_version,
            ));
        }
        if snapshot.latest_event_seq == 0 {
            return Err(TaskRuntimeError::InvalidSnapshotSequence);
        }
        validation::validate_execution_context(&snapshot.execution_context)?;
        validate_output_range(&snapshot.output.stdout)?;
        validate_output_range(&snapshot.output.stderr)?;
        if snapshot.state.is_terminal() != snapshot.finished_at_unix_ms.is_some() {
            return Err(TaskRuntimeError::InvalidFinishedTime);
        }
        if (snapshot.state == TaskState::Succeeded) != snapshot.completion.is_some() {
            return Err(TaskRuntimeError::InvalidCompletion);
        }
        if (snapshot.state == TaskState::Failed) != snapshot.error.is_some() {
            return Err(TaskRuntimeError::InvalidTaskError);
        }
        if let Some(progress) = snapshot.progress.as_ref() {
            validation::progress(None, progress)?;
        }
        Ok(Self { snapshot })
    }

    pub const fn snapshot(&self) -> &TaskSnapshot {
        &self.snapshot
    }

    pub fn record(
        &mut self,
        kind: TaskEventKind,
        occurred_at_unix_ms: i64,
    ) -> Result<TaskEvent, TaskRuntimeError> {
        if self.snapshot.state.is_terminal() {
            return Err(TaskRuntimeError::TerminalTask(self.snapshot.state));
        }
        let mut next = self.snapshot.clone();
        apply(&mut next, &kind, occurred_at_unix_ms)?;
        let seq = next
            .latest_event_seq
            .checked_add(1)
            .ok_or(TaskRuntimeError::SequenceExhausted)?;
        next.latest_event_seq = seq;
        let event = TaskEvent {
            schema_version: TASK_SCHEMA_VERSION,
            task_ref: next.task_ref,
            seq,
            occurred_at_unix_ms,
            kind,
        };
        self.snapshot = next;
        Ok(event)
    }

    pub fn apply_event(&mut self, event: &TaskEvent) -> Result<EventApply, TaskRuntimeError> {
        if event.schema_version != TASK_SCHEMA_VERSION {
            return Err(TaskRuntimeError::UnsupportedSchemaVersion(
                event.schema_version,
            ));
        }
        if event.task_ref != self.snapshot.task_ref {
            return Err(TaskRuntimeError::WrongTask);
        }
        if event.seq <= self.snapshot.latest_event_seq {
            return Ok(EventApply::Duplicate);
        }
        let expected = self
            .snapshot
            .latest_event_seq
            .checked_add(1)
            .ok_or(TaskRuntimeError::SequenceExhausted)?;
        if event.seq != expected {
            return Err(TaskRuntimeError::EventGap {
                expected,
                actual: event.seq,
            });
        }
        if self.snapshot.state.is_terminal() {
            return Err(TaskRuntimeError::TerminalTask(self.snapshot.state));
        }
        let mut next = self.snapshot.clone();
        apply(&mut next, &event.kind, event.occurred_at_unix_ms)?;
        next.latest_event_seq = event.seq;
        self.snapshot = next;
        Ok(EventApply::Applied)
    }

    pub fn observe_output(
        &mut self,
        stream: OutputStream,
        retained_from: u64,
        available_to: u64,
        complete: bool,
    ) -> Result<(), TaskRuntimeError> {
        if retained_from > available_to {
            return Err(TaskRuntimeError::InvalidOutputRange);
        }
        let previous = match stream {
            OutputStream::Stdout => &self.snapshot.output.stdout,
            OutputStream::Stderr => &self.snapshot.output.stderr,
        };
        if retained_from < previous.retained_from || available_to < previous.available_to {
            return Err(TaskRuntimeError::OutputRangeRegressed);
        }
        if previous.complete
            && (!complete
                || retained_from != previous.retained_from
                || available_to != previous.available_to)
        {
            return Err(TaskRuntimeError::CompletedOutputChanged);
        }
        let next = OutputRange {
            retained_from,
            available_to,
            complete,
        };
        match stream {
            OutputStream::Stdout => self.snapshot.output.stdout = next,
            OutputStream::Stderr => self.snapshot.output.stderr = next,
        }
        Ok(())
    }
}

fn validate_output_range(range: &OutputRange) -> Result<(), TaskRuntimeError> {
    if range.retained_from > range.available_to {
        return Err(TaskRuntimeError::InvalidOutputRange);
    }
    Ok(())
}

fn apply(
    snapshot: &mut TaskSnapshot,
    kind: &TaskEventKind,
    occurred_at_unix_ms: i64,
) -> Result<(), TaskRuntimeError> {
    let from = snapshot.state;
    match kind {
        TaskEventKind::Accepted => return Err(TaskRuntimeError::DuplicateAccepted),
        TaskEventKind::Running if from == TaskState::Accepted => {
            snapshot.state = TaskState::Running;
            snapshot.started_at_unix_ms = Some(occurred_at_unix_ms);
        }
        TaskEventKind::CancelRequested
            if matches!(from, TaskState::Accepted | TaskState::Running) =>
        {
            snapshot.state = TaskState::CancelRequested;
        }
        TaskEventKind::Succeeded { completion }
            if matches!(from, TaskState::Running | TaskState::CancelRequested) =>
        {
            snapshot.state = TaskState::Succeeded;
            snapshot.completion = Some(completion.clone());
            snapshot.finished_at_unix_ms = Some(occurred_at_unix_ms);
        }
        TaskEventKind::Failed { error }
            if matches!(
                from,
                TaskState::Accepted | TaskState::Running | TaskState::CancelRequested
            ) =>
        {
            snapshot.state = TaskState::Failed;
            snapshot.error = Some(error.clone());
            snapshot.finished_at_unix_ms = Some(occurred_at_unix_ms);
        }
        TaskEventKind::Cancelled { .. } if from == TaskState::CancelRequested => {
            snapshot.state = TaskState::Cancelled;
            snapshot.finished_at_unix_ms = Some(occurred_at_unix_ms);
        }
        TaskEventKind::Interrupted { .. }
            if matches!(
                from,
                TaskState::Accepted | TaskState::Running | TaskState::CancelRequested
            ) =>
        {
            snapshot.state = TaskState::Interrupted;
            snapshot.finished_at_unix_ms = Some(occurred_at_unix_ms);
        }
        TaskEventKind::StageChanged { stage } => {
            if stage.as_ref().is_some_and(|value| value.trim().is_empty()) {
                return Err(TaskRuntimeError::EmptyField("stage"));
            }
            snapshot.stage = stage.clone();
        }
        TaskEventKind::Progress { progress } => {
            if !matches!(from, TaskState::Running | TaskState::CancelRequested) {
                return Err(TaskRuntimeError::InvalidTransition {
                    from,
                    event: "progress",
                });
            }
            validation::progress(snapshot.progress.as_ref(), progress)?;
            snapshot.progress = Some(progress.clone());
        }
        _ => {
            return Err(TaskRuntimeError::InvalidTransition {
                from,
                event: event_name(kind),
            });
        }
    }
    Ok(())
}

fn event_name(kind: &TaskEventKind) -> &'static str {
    match kind {
        TaskEventKind::Accepted => "accepted",
        TaskEventKind::Running => "running",
        TaskEventKind::StageChanged { .. } => "stage_changed",
        TaskEventKind::Progress { .. } => "progress",
        TaskEventKind::CancelRequested => "cancel_requested",
        TaskEventKind::Succeeded { .. } => "succeeded",
        TaskEventKind::Failed { .. } => "failed",
        TaskEventKind::Cancelled { .. } => "cancelled",
        TaskEventKind::Interrupted { .. } => "interrupted",
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TaskRuntimeError {
    #[error("task schema version {0} is not supported")]
    UnsupportedSchemaVersion(u16),
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
    #[error("{field} must contain at most {max_chars} characters")]
    FieldTooLong {
        field: &'static str,
        max_chars: usize,
    },
    #[error("capability version must be greater than zero")]
    ZeroCapabilityVersion,
    #[error("path style does not match the target OS")]
    PathStyleMismatch,
    #[error("accepted can only be recorded when the task is created")]
    DuplicateAccepted,
    #[error("task is already terminal in state {0:?}")]
    TerminalTask(TaskState),
    #[error("event sequence is exhausted")]
    SequenceExhausted,
    #[error("a restored snapshot must have at least the accepted event")]
    InvalidSnapshotSequence,
    #[error("terminal state and finished time are inconsistent")]
    InvalidFinishedTime,
    #[error("only a succeeded task may contain completion data")]
    InvalidCompletion,
    #[error("only a failed task may contain task error data")]
    InvalidTaskError,
    #[error("event belongs to a different task")]
    WrongTask,
    #[error("event stream has a gap: expected {expected}, received {actual}")]
    EventGap { expected: u64, actual: u64 },
    #[error("{event} is invalid from state {from:?}")]
    InvalidTransition {
        from: TaskState,
        event: &'static str,
    },
    #[error("confirmed bytes exceed the declared total")]
    ProgressExceedsTotal,
    #[error("confirmed bytes must not decrease")]
    ProgressRegressed,
    #[error("transfer phase must not move backwards")]
    TransferPhaseRegressed,
    #[error("transfer direction must remain unchanged")]
    TransferDirectionChanged,
    #[error("a declared transfer total must remain unchanged")]
    TransferTotalChanged,
    #[error("a declared transfer total must not be removed")]
    TransferTotalRemoved,
    #[error("output retained offset must not exceed its available end")]
    InvalidOutputRange,
    #[error("output offsets must not move backwards")]
    OutputRangeRegressed,
    #[error("a completed output stream must remain immutable")]
    CompletedOutputChanged,
}
