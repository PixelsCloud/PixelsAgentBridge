use pab_protocol::{CommandTaskSpec, OutputStream, TaskError, TaskEventKind, TaskRef};
use tokio::sync::watch;

use super::{TaskService, TaskServiceError, unix_millis};
use crate::task_store::TaskStore;

impl TaskService {
    pub(super) async fn finalize_user_worker_error(
        &self,
        task_ref: TaskRef,
    ) -> Result<(), TaskServiceError> {
        self.finish_outputs(task_ref).await?;
        self.store.record_event(task_ref,TaskEventKind::Interrupted{reason:"user_worker_result_unconfirmed: user worker ended without a confirmed result; inspect the original task before retrying".into()},unix_millis()?).await?;
        Ok(())
    }
    pub(super) async fn run_command(
        &self,
        task_ref: TaskRef,
        command: CommandTaskSpec,
        prepared: Option<pab_os_control::execution::PreparedUser>,
        cancel: watch::Receiver<Option<String>>,
    ) -> Result<(), TaskServiceError> {
        let sink = CommandSink::Store {
            store: self.store.clone(),
            task_ref,
        };
        if let Some(prepared) = prepared {
            let executable = self.user_worker_executable()?;
            crate::user_worker::execute(&executable, prepared, command, sink, cancel).await
        } else {
            execute(sink, command, cancel).await
        }
    }

    pub(super) async fn finalize_worker_error(
        &self,
        task_ref: TaskRef,
    ) -> Result<(), TaskServiceError> {
        self.finish_outputs(task_ref).await?;
        self.store
            .record_event(
                task_ref,
                TaskEventKind::Failed {
                    error: TaskError {
                        code: "executor_internal_error".to_owned(),
                        message: "Executor could not complete the command task".to_owned(),
                        exit_code: None,
                    },
                },
                unix_millis()?,
            )
            .await?;
        Ok(())
    }

    pub(super) async fn finish_outputs(&self, task_ref: TaskRef) -> Result<(), TaskServiceError> {
        self.store
            .complete_output(task_ref, OutputStream::Stdout)
            .await?;
        self.store
            .complete_output(task_ref, OutputStream::Stderr)
            .await?;
        Ok(())
    }
}

pub(crate) async fn execute(
    sink: CommandSink,
    command: CommandTaskSpec,
    cancel: watch::Receiver<Option<String>>,
) -> Result<(), TaskServiceError> {
    pab_command_runtime::execute(sink, command, cancel).await
}

impl pab_command_runtime::CommandSink for CommandSink {
    type Error = TaskServiceError;
    async fn event(&self, event: TaskEventKind) -> Result<(), Self::Error> {
        self.event(event).await
    }
    async fn output(&self, stream: OutputStream, bytes: &[u8]) -> Result<(), Self::Error> {
        self.output(stream, bytes).await
    }
    async fn finish_outputs(&self) -> Result<(), Self::Error> {
        self.finish_outputs().await
    }
}

#[derive(Clone)]
pub(crate) enum CommandSink {
    Store { store: TaskStore, task_ref: TaskRef },
    Worker(tokio::sync::mpsc::Sender<CommandMessage>),
}
pub(crate) enum CommandMessage {
    Event(TaskEventKind),
    Output(OutputStream, Vec<u8>),
    Complete(OutputStream),
}
impl CommandSink {
    pub(crate) async fn send(&self, message: CommandMessage) -> Result<(), TaskServiceError> {
        match self {
            Self::Store { store, task_ref } => match message {
                CommandMessage::Event(event) => {
                    store.record_event(*task_ref, event, unix_millis()?).await?;
                }
                CommandMessage::Output(stream, bytes) => {
                    store.append_output(*task_ref, stream, &bytes).await?;
                }
                CommandMessage::Complete(stream) => {
                    store.complete_output(*task_ref, stream).await?;
                }
            },
            Self::Worker(sender) => sender.send(message).await.map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "user worker output channel closed",
                )
            })?,
        }
        Ok(())
    }
    pub(crate) async fn event(&self, event: TaskEventKind) -> Result<(), TaskServiceError> {
        self.send(CommandMessage::Event(event)).await
    }
    async fn output(&self, stream: OutputStream, bytes: &[u8]) -> Result<(), TaskServiceError> {
        self.send(CommandMessage::Output(stream, bytes.to_vec()))
            .await
    }
    async fn finish_outputs(&self) -> Result<(), TaskServiceError> {
        self.send(CommandMessage::Complete(OutputStream::Stdout))
            .await?;
        self.send(CommandMessage::Complete(OutputStream::Stderr))
            .await
    }
}
