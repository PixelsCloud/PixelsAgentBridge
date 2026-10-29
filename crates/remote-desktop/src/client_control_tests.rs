use super::*;
use pab_transport::{PabEndpoint, PabEndpointAddress, PabEndpointConfig};
const WAIT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn ack_trace_separates_local_queue_write_and_delayed_reply() {
    let mut h = Harness::new(WAIT).await;
    h.present(1);
    let request = h.next().await;
    assert!(matches!(request.command, DesktopControlCommand::Presented { frame_id: 1, .. }));
    let before = h.ack_history.lock().unwrap().snapshot(1).remove(0);
    assert!(before.queue_us.is_some());
    assert!(before.write_us.is_some());
    assert!(before.reply_us.is_none());
    tokio::time::sleep(Duration::from_millis(50)).await;
    h.reply(request.sequence).await;
    let after = tokio::time::timeout(WAIT, async {
        loop {
            let after = h.ack_history.lock().unwrap().snapshot(1).remove(0);
            if after.reply_us.is_some() { break after; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    assert!(after.reply_us.unwrap() >= 50_000);
    assert_eq!(after.queue_us, before.queue_us);
    assert_eq!(after.write_us, before.write_us);
}

async fn pair() -> (PabEndpoint, PabEndpoint, PabConnection, PabConnection) {
    let config = PabEndpointConfig::new(vec!["https://127.0.0.1:1".parse().unwrap()]).unwrap();
    let a = PabEndpoint::bind(config.clone(), iroh_base::SecretKey::generate())
        .await
        .unwrap();
    let b = PabEndpoint::bind(config, iroh_base::SecretKey::generate())
        .await
        .unwrap();
    let mut addresses = b.watch_address();
    tokio::time::timeout(WAIT, async {
        while addresses.borrow().direct_addresses.is_empty() {
            addresses.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let direct_addresses = addresses
        .borrow()
        .direct_addresses
        .iter()
        .map(|a| {
            std::net::SocketAddr::new(
                if a.is_ipv4() {
                    std::net::Ipv4Addr::LOCALHOST.into()
                } else {
                    std::net::Ipv6Addr::LOCALHOST.into()
                },
                a.port(),
            )
        })
        .collect();
    let remote = b.clone();
    let accept = tokio::spawn(async move { remote.accept().await.unwrap().unwrap() });
    let client = a
        .connect(
            *b.id().as_bytes(),
            &PabEndpointAddress {
                direct_addresses,
                relay_urls: vec![],
            },
            WAIT,
        )
        .await
        .unwrap();
    (a, b, client, accept.await.unwrap())
}

struct Harness {
    _a: PabEndpoint,
    _b: PabEndpoint,
    _client: PabConnection,
    _peer: PabConnection,
    commands: mpsc::Sender<Command>,
    presentations: watch::Sender<Option<(u64, u64)>>,
    ack_history: Arc<Mutex<ack_trace::AckHistory>>,
    requests: mpsc::Receiver<DesktopControl>,
    writer: PabFrameWriter,
    worker: tokio::task::JoinHandle<()>,
    reader: tokio::task::JoinHandle<()>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.worker.abort();
        self.reader.abort();
    }
}
impl Harness {
    async fn new(timeout: Duration) -> Self {
        Self::with_heartbeat(timeout, true).await
    }
    async fn with_heartbeat(timeout: Duration, confirm: bool) -> Self {
        let (a, b, client, peer) = pair().await;
        let stream = client.open_bi(WAIT).await.unwrap();
        let (commands, command_rx) = mpsc::channel(4);
        let (presentations, presented) = watch::channel(None);
        let ack_history = Arc::new(Mutex::new(ack_trace::AckHistory::default()));
        let worker = tokio::spawn(control_worker(
            stream,
            command_rx,
            presented,
            ack_history.clone(),
            Duration::from_secs(60),
            timeout,
        ));
        let stream = peer.accept_bi(WAIT).await.unwrap();
        let (writer, mut reader) = stream.into_framed();
        let (requests, request_rx) = mpsc::channel(8);
        // Keep the real framed read alive across every timeout assertion. Only
        // the channel receive is cancelled; no partial packet is ever reused.
        let reader = tokio::spawn(async move {
            while let Ok(bytes) = reader.read(DESKTOP_CONTROL_BYTES).await {
                let request = serde_json::from_slice(&bytes).unwrap();
                if requests.send(request).await.is_err() {
                    break;
                }
            }
        });
        let mut h = Self {
            _a: a,
            _b: b,
            _client: client,
            _peer: peer,
            commands,
            presentations,
            ack_history,
            requests: request_rx,
            writer,
            worker,
            reader,
        };
        let first = h.next().await;
        assert!(matches!(first.command, DesktopControlCommand::Heartbeat));
        if confirm {
            h.reply(first.sequence).await;
        }
        h
    }
    async fn next(&mut self) -> DesktopControl {
        tokio::time::timeout(WAIT, self.requests.recv())
            .await
            .unwrap()
            .unwrap()
    }
    async fn reply(&mut self, sequence: u64) {
        self.writer
            .write(
                &serde_json::to_vec(&DesktopControlReply::Applied { sequence }).unwrap(),
                DESKTOP_CONTROL_BYTES,
            )
            .await
            .unwrap();
    }
    fn present(&self, frame_id: u64) {
        self.ack_history.lock().unwrap().notified(1, frame_id);
        self.presentations.send_replace(Some((1, frame_id)));
    }
    fn disable_input(&self) -> oneshot::Receiver<Result<DesktopControlReply>> {
        let (result, reply) = oneshot::channel();
        self.commands
            .try_send(Command {
                command: DesktopControlCommand::SetInput {
                    generation: 1,
                    enabled: false,
                },
                result,
            })
            .unwrap();
        reply
    }
    async fn no_request_yet(&mut self) {
        assert!(
            tokio::time::timeout(Duration::from_millis(40), self.requests.recv())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn display_credit_does_not_wait_for_a_heartbeat_reply() {
    let mut h = Harness::with_heartbeat(WAIT, false).await;
    h.present(1);
    let request = tokio::time::timeout(Duration::from_millis(250), h.requests.recv())
        .await
        .expect("video credit was blocked by the heartbeat round trip")
        .unwrap();
    assert_eq!(request.sequence, 2);
    assert!(matches!(
        request.command,
        DesktopControlCommand::Presented { frame_id: 1, .. }
    ));
}

#[tokio::test]
async fn display_credit_continues_while_ordered_input_waits_for_its_reply() {
    let mut h = Harness::new(WAIT).await;
    let first_result = h.disable_input();
    let first = h.next().await;
    let second_result = h.disable_input();
    h.present(1);
    let ack = tokio::time::timeout(Duration::from_millis(250), h.requests.recv())
        .await
        .expect("video credit was blocked by the input round trip")
        .unwrap();
    assert_eq!(ack.sequence, first.sequence + 1);
    assert!(matches!(
        ack.command,
        DesktopControlCommand::Presented { frame_id: 1, .. }
    ));
    h.no_request_yet().await;
    // The next input must still wait for the previous input's definitive reply.
    h.reply(first.sequence).await;
    assert!(first_result.await.unwrap().is_ok());
    let second = h.next().await;
    assert_eq!(second.sequence, ack.sequence + 1);
    assert!(matches!(
        second.command,
        DesktopControlCommand::SetInput { enabled: false, .. }
    ));
    h.reply(ack.sequence).await;
    h.reply(second.sequence).await;
    assert!(second_result.await.unwrap().is_ok());
}

#[tokio::test]
async fn input_and_acks_share_the_same_bounded_window() {
    let mut h = Harness::new(WAIT).await;
    let _result = h.disable_input();
    let input = h.next().await;
    for id in 1..control::ACK_WINDOW as u64 {
        h.present(id);
        assert!(
            matches!(h.next().await.command, DesktopControlCommand::Presented { frame_id, .. } if frame_id == id)
        );
    }
    h.present(control::ACK_WINDOW as u64);
    h.no_request_yet().await;
    h.reply(input.sequence).await;
    assert!(
        matches!(h.next().await.command, DesktopControlCommand::Presented { frame_id, .. } if frame_id == control::ACK_WINDOW as u64)
    );
}

#[tokio::test]
async fn screen_switch_still_blocks_old_generation_credit() {
    let mut h = Harness::new(WAIT).await;
    let (result, _reply) = oneshot::channel();
    h.commands
        .try_send(Command {
            command: DesktopControlCommand::SelectScreen {
                generation: 1,
                screen_id: "test-screen".into(),
            },
            result,
        })
        .unwrap();
    let switch = h.next().await;
    h.present(1);
    h.no_request_yet().await;
    // The session clears the old presentation channel before installing the new generation.
    h.presentations.send_replace(None);
    h.reply(switch.sequence).await;
    h.no_request_yet().await;
}

#[tokio::test]
async fn later_display_ack_can_arrive_before_an_earlier_reply() {
    let mut h = Harness::new(WAIT).await;
    h.present(1);
    let first = h.next().await;
    assert!(matches!(
        first.command,
        DesktopControlCommand::Presented { frame_id: 1, .. }
    ));
    h.present(2);
    // Deliberately withhold the first reply. The serial implementation failed
    // this assertion; it could not return fresh video credit before one RTT.
    let second = tokio::time::timeout(Duration::from_millis(250), h.requests.recv())
        .await
        .expect("display ACK was blocked on a previous reply")
        .unwrap();
    assert_eq!(second.sequence, first.sequence + 1);
    assert!(matches!(
        second.command,
        DesktopControlCommand::Presented { frame_id: 2, .. }
    ));
}

#[tokio::test]
async fn ack_window_is_bounded() {
    let mut h = Harness::new(WAIT).await;
    let mut sequences = Vec::new();
    for frame_id in 1..=control::ACK_WINDOW as u64 {
        h.present(frame_id);
        let request = h.next().await;
        assert!(
            matches!(request.command, DesktopControlCommand::Presented { frame_id: id, .. } if id == frame_id)
        );
        sequences.push(request.sequence);
    }
    h.present(control::ACK_WINDOW as u64 + 1);
    h.no_request_yet().await;
    h.reply(sequences[0]).await;
    let latest = h.next().await;
    assert_eq!(latest.sequence, sequences.last().unwrap() + 1);
    assert!(matches!(
        latest.command,
        DesktopControlCommand::Presented { frame_id, .. } if frame_id == control::ACK_WINDOW as u64 + 1
    ));
    h.present(control::ACK_WINDOW as u64 + 2);
    h.no_request_yet().await;
}

#[tokio::test]
async fn mismatched_ack_reply_retires_the_stream_without_replaying_queued_input() {
    let mut h = Harness::new(WAIT).await;
    h.present(1);
    let request = h.next().await;
    let input = h.disable_input();
    let mutation = h.next().await;
    assert!(matches!(
        mutation.command,
        DesktopControlCommand::SetInput { .. }
    ));
    h.reply(request.sequence + 100).await;
    tokio::time::timeout(WAIT, &mut h.worker)
        .await
        .unwrap()
        .unwrap();
    assert!(input.await.is_err());
    assert!(h.commands.is_closed());
    assert!(h.presentations.is_closed());
    assert!(h.requests.try_recv().is_err());
}

#[tokio::test]
async fn stalled_ack_reply_keeps_the_existing_timeout_and_cancels_queued_input() {
    let mut h = Harness::new(Duration::from_millis(250)).await;
    h.present(1);
    let _request = h.next().await;
    let input = h.disable_input();
    let mutation = h.next().await;
    assert!(matches!(
        mutation.command,
        DesktopControlCommand::SetInput { .. }
    ));
    tokio::time::timeout(Duration::from_secs(2), &mut h.worker)
        .await
        .unwrap()
        .unwrap();
    assert!(input.await.is_err());
    assert!(h.commands.is_closed());
    assert!(h.presentations.is_closed());
    assert!(h.requests.try_recv().is_err());
}
