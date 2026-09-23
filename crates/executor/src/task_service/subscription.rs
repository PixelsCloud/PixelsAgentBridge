use std::time::Duration;

use pab_protocol::{DeviceTaskResponse, MAX_OUTPUT_READ_BYTES, OperatorRef, OutputStream, TaskRef};
use pab_transport::PabBiStream;
use tokio::sync::broadcast;

use super::{TaskService, TaskServiceError, error_response};

impl TaskService {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn subscribe_stream(
        &self,
        initiated_by: OperatorRef,
        task_ref: TaskRef,
        mut event_seq: u64,
        mut stdout_offset: u64,
        mut stderr_offset: u64,
        mut stream: PabBiStream,
        timeout: Duration,
    ) -> Result<(), TaskServiceError> {
        self.validate_task_ref(task_ref)?;
        let mut changes = self.store.subscribe();
        if let Err(error) = self.store.get_task(initiated_by, task_ref).await {
            let response = error_response(&TaskServiceError::Store(error));
            stream.send_json(&response, timeout).await?;
            return Ok(());
        }
        self.catch_up(
            initiated_by,
            task_ref,
            &mut event_seq,
            &mut stdout_offset,
            &mut stderr_offset,
            &mut stream,
            timeout,
        )
        .await?;
        let snapshot = self.store.get_task(initiated_by, task_ref).await?;
        if snapshot.state.is_terminal()
            && snapshot.output.stdout.complete
            && snapshot.output.stderr.complete
        {
            stream.finish_send(timeout).await?;
            return Ok(());
        }
        loop {
            match changes.recv().await {
                Ok(change) if change.task_ref == task_ref => {
                    self.catch_up(
                        initiated_by,
                        task_ref,
                        &mut event_seq,
                        &mut stdout_offset,
                        &mut stderr_offset,
                        &mut stream,
                        timeout,
                    )
                    .await?;
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    self.catch_up(
                        initiated_by,
                        task_ref,
                        &mut event_seq,
                        &mut stdout_offset,
                        &mut stderr_offset,
                        &mut stream,
                        timeout,
                    )
                    .await?;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
            let snapshot = self.store.get_task(initiated_by, task_ref).await?;
            if snapshot.state.is_terminal()
                && snapshot.output.stdout.complete
                && snapshot.output.stderr.complete
            {
                stream.finish_send(timeout).await?;
                return Ok(());
            }
        }
        stream.finish_send(timeout).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn catch_up(
        &self,
        initiated_by: OperatorRef,
        task_ref: TaskRef,
        event_seq: &mut u64,
        stdout_offset: &mut u64,
        stderr_offset: &mut u64,
        stream: &mut PabBiStream,
        timeout: Duration,
    ) -> Result<(), TaskServiceError> {
        loop {
            for event in self
                .store
                .events_after(initiated_by, task_ref, *event_seq)
                .await?
            {
                *event_seq = event.seq;
                stream
                    .send_frame_json(&DeviceTaskResponse::Event { event }, timeout)
                    .await?;
            }
            self.catch_up_output(
                initiated_by,
                task_ref,
                OutputStream::Stdout,
                stdout_offset,
                stream,
                timeout,
            )
            .await?;
            self.catch_up_output(
                initiated_by,
                task_ref,
                OutputStream::Stderr,
                stderr_offset,
                stream,
                timeout,
            )
            .await?;
            let snapshot = self.store.get_task(initiated_by, task_ref).await?;
            if *event_seq < snapshot.latest_event_seq
                || *stdout_offset < snapshot.output.stdout.available_to
                || *stderr_offset < snapshot.output.stderr.available_to
            {
                continue;
            }
            stream
                .send_frame_json(
                    &DeviceTaskResponse::CaughtUp {
                        snapshot: Box::new(snapshot),
                    },
                    timeout,
                )
                .await?;
            return Ok(());
        }
    }

    async fn catch_up_output(
        &self,
        initiated_by: OperatorRef,
        task_ref: TaskRef,
        output_stream: OutputStream,
        offset: &mut u64,
        stream: &mut PabBiStream,
        timeout: Duration,
    ) -> Result<(), TaskServiceError> {
        loop {
            let (chunk, range) = self
                .store
                .read_output(
                    initiated_by,
                    task_ref,
                    output_stream,
                    *offset,
                    MAX_OUTPUT_READ_BYTES,
                )
                .await?;
            if chunk.bytes.is_empty() {
                if range.complete {
                    stream
                        .send_frame_json(
                            &DeviceTaskResponse::OutputChanged {
                                task_ref,
                                stream: output_stream,
                                range,
                            },
                            timeout,
                        )
                        .await?;
                }
                break;
            }
            *offset = offset
                .checked_add(u64::try_from(chunk.bytes.len()).map_err(|_| TaskServiceError::Clock)?)
                .ok_or(TaskServiceError::Clock)?;
            stream
                .send_frame_json(&DeviceTaskResponse::Output { chunk, range }, timeout)
                .await?;
        }
        Ok(())
    }
}
