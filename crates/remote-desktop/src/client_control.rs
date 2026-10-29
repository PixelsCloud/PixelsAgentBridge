//! Pipeline bounded display ACKs, while keeping mutations ordered. A delayed
//! heartbeat or input reply must not prevent returning newer video
//! credit. The reader owns each complete reply until success or stream teardown.
use super::*;
use std::collections::VecDeque;
use tokio::time::Instant;

// A reply travels a full RTT, while returning video credit needs only its
// outbound half. Allow the previous ACK window's replies to remain in flight
// while returning the next window of credit. No video payload is queued here.
pub(super) const ACK_WINDOW: usize = 2 * DESKTOP_MAX_IN_FLIGHT as usize;
struct Pending {
    sequence: u64,
    action: &'static str,
    response: Option<oneshot::Sender<Result<DesktopControlReply>>>,
    deadline: Instant,
    started: Instant,
    presentation: Option<(u64, u64)>,
}
struct ReplyReader(tokio::task::JoinHandle<()>);
impl Drop for ReplyReader {
    fn drop(&mut self) {
        self.0.abort();
    }
}
enum Event {
    Reply(Option<Result<DesktopControlReply>>),
    Command(Option<Command>),
    Presentation(std::result::Result<(), watch::error::RecvError>),
    Heartbeat,
    Expired,
}
fn action(command: &DesktopControlCommand) -> &'static str {
    match command {
        DesktopControlCommand::Heartbeat => "heartbeat",
        DesktopControlCommand::Presented { .. } => "presentation_ack",
        DesktopControlCommand::SelectScreen { .. } => "select_screen",
        DesktopControlCommand::SetInput { .. } => "set_input",
        DesktopControlCommand::Input { .. } => "input",
    }
}
fn failed(pending: &Pending, error: &DesktopError) {
    tracing::warn!(sequence = pending.sequence, action = pending.action, %error,
        "remote desktop control channel failed");
}

pub(super) async fn run(
    stream: PabBiStream,
    mut commands: mpsc::Receiver<Command>,
    mut presentations: watch::Receiver<Option<(u64, u64)>>,
    ack_history: Arc<Mutex<ack_trace::AckHistory>>,
    heartbeat: Duration,
    timeout: Duration,
) {
    let (mut writer, mut reader) = stream.into_framed();
    let (replies, mut incoming) = mpsc::channel(ACK_WINDOW);
    let _reader = ReplyReader(tokio::spawn(async move {
        loop {
            let reply = async {
                Ok(serde_json::from_slice::<DesktopControlReply>(
                    &reader.read(DESKTOP_CONTROL_BYTES).await?,
                )?)
            }
            .await;
            let ended = reply.is_err();
            if replies.send(reply).await.is_err() || ended {
                break;
            }
        }
    }));
    let mut ticks = tokio::time::interval(heartbeat);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut sequence = 0_u64;
    let mut pending: VecDeque<Pending> = VecDeque::with_capacity(ACK_WINDOW);
    let mut queued: Option<Command> = None;
    loop {
        let mutation_pending = pending
            .iter()
            .any(|p| !matches!(p.action, "presentation_ack" | "heartbeat"));
        let outgoing = if queued.is_some() && pending.len() < ACK_WINDOW && !mutation_pending {
            let command = queued.take().unwrap();
            Some((command.command, Some(command.result)))
        } else {
            // ACKs do not mutate input state. Keep returning video credit while
            // an earlier input/heartbeat reply travels back. A screen switch
            // alone retires that generation's presentation notifications.
            let can_ack = pending.len() < ACK_WINDOW
                && !pending.iter().any(|p| p.action == "select_screen")
                && !queued.as_ref().is_some_and(|c| {
                    matches!(c.command, DesktopControlCommand::SelectScreen { .. })
                });
            let deadline = pending.front().map(|p| p.deadline);
            let event = tokio::select! {
                biased;
                // Drain replies before spending more control credit.
                reply = incoming.recv() => Event::Reply(reply),
                _ = async {
                    match deadline {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending::<()>().await,
                    }
                } => Event::Expired,
                changed = presentations.changed(), if can_ack => Event::Presentation(changed),
                command = commands.recv(), if queued.is_none() => Event::Command(command),
                _ = ticks.tick(), if pending.is_empty() && queued.is_none() => Event::Heartbeat,
            };
            match event {
                Event::Reply(reply) => {
                    // A fast presentation ACK may overtake a slow input reply.
                    // Match an outstanding sequence exactly; never accept a
                    // duplicate/unknown reply or let it complete another caller.
                    let index = match &reply {
                        Some(Ok(reply)) => pending.iter().position(|p| p.sequence == reply.sequence()),
                        _ => (!pending.is_empty()).then_some(0),
                    };
                    let Some(index) = index else {
                        tracing::warn!("remote desktop control reply has no pending sequence");
                        break;
                    };
                    let mut sent = pending.remove(index).unwrap();
                    let result = match reply {
                        Some(Ok(reply)) if reply.sequence() != sent.sequence => Err(
                            DesktopError::Protocol("desktop control reply sequence mismatch"),
                        ),
                        Some(Ok(DesktopControlReply::Error { code, message, .. })) => {
                            Err(DesktopError::reject(code, message))
                        }
                        Some(result) => result,
                        None => Err(DesktopError::Closed),
                    };
                    let fatal = result.as_ref().is_err_and(|error| {
                        sent.response.is_none() || !matches!(error, DesktopError::Rejected { .. })
                    });
                    if result.is_ok() {
                        if let Some(id) = sent.presentation {
                            ack_history.lock().unwrap().replied(id, sent.started.elapsed());
                        }
                    }
                    if fatal {
                        if let Err(error) = &result {
                            failed(&sent, error);
                        }
                    }
                    if let Some(response) = sent.response.take() {
                        let _ = response.send(result);
                    }
                    if fatal {
                        break;
                    }
                    continue;
                }
                Event::Command(Some(command)) => {
                    queued = Some(command);
                    continue;
                }
                Event::Command(None) | Event::Presentation(Err(_)) => break,
                Event::Presentation(Ok(())) => {
                    let latest = *presentations.borrow_and_update();
                    let Some((generation, frame_id)) = latest else {
                        continue;
                    };
                    Some((
                        DesktopControlCommand::Presented {
                            generation,
                            frame_id,
                        },
                        None,
                    ))
                }
                Event::Heartbeat => Some((DesktopControlCommand::Heartbeat, None)),
                Event::Expired => {
                    if let Some(mut sent) = pending.pop_front() {
                        failed(&sent, &DesktopError::Timeout);
                        if let Some(response) = sent.response.take() {
                            let _ = response.send(Err(DesktopError::Timeout));
                        }
                    }
                    break;
                }
            }
        };
        let Some((command, response)) = outgoing else {
            continue;
        };
        let Some(next) = sequence.checked_add(1) else {
            break;
        };
        sequence = next;
        let presentation = match &command {
            DesktopControlCommand::Presented { generation, frame_id } => Some((*generation, *frame_id)),
            _ => None,
        };
        let started = Instant::now();
        if let Some(id) = presentation { ack_history.lock().unwrap().sending(id, pending.len()); }
        let mut sent = Pending {
            sequence,
            action: action(&command),
            response,
            deadline: Instant::now() + timeout,
            started,
            presentation,
        };
        let request = DesktopControl { sequence, command };
        let write = async {
            writer
                .write(&serde_json::to_vec(&request)?, DESKTOP_CONTROL_BYTES)
                .await?;
            Result::<()>::Ok(())
        };
        let result = match tokio::time::timeout_at(sent.deadline, write).await {
            Ok(result) => result,
            Err(_) => Err(DesktopError::Timeout),
        };
        if let Err(error) = result {
            failed(&sent, &error);
            if let Some(response) = sent.response.take() {
                let _ = response.send(Err(error));
            }
            break;
        }
        if let Some(id) = presentation { ack_history.lock().unwrap().written(id, started.elapsed()); }
        pending.push_back(sent);
    }
    // Dropping the queue closes remaining callers; dropping ReplyReader aborts
    // the only partial read and retires it. Never retry or replay a command.
}
