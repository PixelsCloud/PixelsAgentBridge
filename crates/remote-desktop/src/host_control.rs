//! Read/acknowledge presentation independently of ordered native input work.
//! Bounded queues, one mutation actor, one framed reader and one framed writer.
use super::*;
use tokio::sync::mpsc;

const QUEUE: usize = 16;
fn output(sequence: u64, result: Result<Option<DesktopOpened>>) -> DesktopControlReply {
    match result {
        Ok(None) => DesktopControlReply::Applied { sequence },
        Ok(Some(opened)) => DesktopControlReply::Configured { sequence, opened },
        Err(error) => DesktopControlReply::Error {
            sequence,
            code: error.code(),
            message: error.to_string(),
        },
    }
}
impl DesktopHost {
    pub(super) async fn control_independent(
        &self,
        entry: &Arc<Entry>,
        stream: PabBiStream,
        timeout: Duration,
    ) -> Result<()> {
        let (mut writer, mut reader) = stream.into_framed();
        let (mutations, mut work) = mpsc::channel::<(DesktopControl, Instant)>(QUEUE);
        let (responses, mut replies) = mpsc::channel::<DesktopControlReply>(QUEUE);
        let input_responses = responses.clone();
        let receive = async {
            let mut sequence = 0u64;
            loop {
                // This is the only reader; no cancellation/retry of partial frames.
                let request: DesktopControl =
                    serde_json::from_slice(&reader.read(DESKTOP_CONTROL_BYTES).await?)?;
                sequence = sequence
                    .checked_add(1)
                    .ok_or(DesktopError::Protocol("control sequence overflow"))?;
                if request.sequence != sequence {
                    return Err(DesktopError::Protocol("out-of-order desktop control"));
                }
                *entry.last_seen.lock().unwrap() = Instant::now();
                let result = match request.command {
                    DesktopControlCommand::Heartbeat => Ok(None),
                    DesktopControlCommand::Presented {
                        generation,
                        frame_id,
                    } => entry
                        .video(generation)
                        .and_then(|v| {
                            v.ensure_live()?;
                            v.acknowledge(frame_id)
                        })
                        .map(|_| None),
                    _ => {
                        // Legitimate clients keep one mutation outstanding.
                        // A flooding peer cannot consume unbounded memory or
                        // keep the read loop waiting behind native OS input.
                        mutations
                            .try_send((request, Instant::now()))
                            .map_err(|_| DesktopError::Protocol("desktop input queue exhausted"))?;
                        continue;
                    }
                };
                responses
                    .send(output(sequence, result))
                    .await
                    .map_err(|_| DesktopError::Closed)?;
            }
        };
        let input = async {
            let mut lease: Option<InputLease> = None;
            loop {
                let generation = lease.as_ref().map(|l| l.generation.clone());
                let release = async {
                    match generation {
                        Some(v) => cancelled(&v.cancelled).await,
                        None => std::future::pending().await,
                    }
                };
                let request = tokio::select! {
                    biased;
                    _ = release => { lease = None; continue; },
                    request = work.recv() => request.ok_or(DesktopError::Closed)?,
                };
                let (request, queued) = request;
                let queue_ms = queued.elapsed().as_millis() as u64;
                let started = Instant::now();
                let (action, result) = match request.command {
                    DesktopControlCommand::SelectScreen {
                        generation,
                        screen_id,
                    } => {
                        lease = None;
                        (
                            "select_screen",
                            self.select_screen(entry, generation, &screen_id)
                                .await
                                .map(Some),
                        )
                    }
                    DesktopControlCommand::SetInput {
                        generation,
                        enabled,
                    } => (
                        "set_input",
                        self.set_input(entry, &mut lease, generation, enabled)
                            .await
                            .map(|_| None),
                    ),
                    DesktopControlCommand::Input { generation, event } => (
                        "input",
                        self.apply_input(entry, &mut lease, generation, event)
                            .await
                            .map(|_| None),
                    ),
                    _ => return Err(DesktopError::Protocol("invalid desktop mutation")),
                };
                let elapsed_ms = started.elapsed().as_millis() as u64;
                if elapsed_ms >= 250 || queue_ms >= 250 || result.is_err() {
                    tracing::warn!(session = %entry.id, sequence = request.sequence, action, queue_ms, elapsed_ms,
                        error_code = ?result.as_ref().err().map(DesktopError::code), "remote desktop input processing delayed or failed");
                }
                input_responses
                    .send(output(request.sequence, result))
                    .await
                    .map_err(|_| DesktopError::Closed)?;
            }
        };
        let send = async {
            while let Some(reply) = replies.recv().await {
                tokio::time::timeout(
                    timeout,
                    writer.write(&serde_json::to_vec(&reply)?, DESKTOP_CONTROL_BYTES),
                )
                .await
                .map_err(|_| DesktopError::Timeout)??;
            }
            Err(DesktopError::Closed)
        };
        // Any stream failure drops all three futures and the held input lease.
        // No detached worker may execute queued input after the session ends.
        tokio::select! { result = receive => result, result = input => result, result = send => result }
    }
}
