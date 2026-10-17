//! Native window events have a separate pipe from decode ACKs. Slow network
//! input cannot block video, and motion is coalesced without dropping key ups.
#![forbid(unsafe_code)]
use pab_protocol::{DesktopMouseButton, RemoteDesktopInput};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{ChildStderr, ChildStdin},
    sync::{Mutex, mpsc, watch},
};
#[derive(Clone, Debug)]
enum Event {
    Input(RemoteDesktopInput),
    Wheel { x: u16, y: u16, delta: i16 },
    Mode(bool),
}
type Stamped = (u64, Event);
pub struct NativeViewerInput {
    input: Control,
    inbox: Inbox,
    reader: Option<tokio::task::JoinHandle<()>>,
    pending: Option<RemoteDesktopInput>,
}
enum Control {
    Pipe(Arc<Mutex<ChildStdin>>),
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    Callback(Arc<dyn Fn(bool) -> io::Result<()> + Send + Sync>),
}
pub(crate) struct InputSink {
    index: AtomicU64,
    events: mpsc::Sender<Stamped>,
    movement: watch::Sender<Option<Stamped>>,
    failure: watch::Sender<bool>,
}
impl InputSink {
    pub(crate) fn fail(&self) {
        self.failure.send_replace(true);
    }
    pub(crate) fn emit(&self, packet: [u8; 8]) -> bool {
        let Some(event) = decode(packet) else {
            self.fail();
            return false;
        };
        let Ok(previous) = self
            .index
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
        else {
            self.fail();
            return false;
        };
        if *self.failure.borrow() || self.events.is_closed() || self.movement.is_closed() {
            return false;
        }
        let stamped = (previous + 1, event);
        if matches!(stamped.1, Event::Input(RemoteDesktopInput::Move { .. })) {
            self.movement.send_replace(Some(stamped));
        } else if self.events.try_send(stamped).is_err() {
            self.fail();
            return false; // Never silently lose a key-up or mode ACK.
        }
        true
    }
    fn channel() -> (Arc<Self>, Inbox) {
        let (events, receive) = mpsc::channel(64);
        let (movement, moves) = watch::channel(None);
        let (failure, failed) = watch::channel(false);
        (
            Arc::new(Self {
                index: AtomicU64::new(0),
                events,
                movement,
                failure,
            }),
            Inbox {
                events: receive,
                movement: moves,
                failed,
                consumed: 0,
            },
        )
    }
}
struct Inbox {
    events: mpsc::Receiver<Stamped>,
    movement: watch::Receiver<Option<Stamped>>,
    failed: watch::Receiver<bool>,
    consumed: u64,
}
impl Drop for NativeViewerInput {
    fn drop(&mut self) {
        if let Some(reader) = &self.reader {
            reader.abort();
        }
    }
}
impl NativeViewerInput {
    pub(crate) fn new(input: Arc<Mutex<ChildStdin>>, mut pipe: ChildStderr) -> Self {
        let (sink, inbox) = InputSink::channel();
        let reader = tokio::spawn(async move {
            loop {
                let mut packet = [0; 8];
                if pipe.read_exact(&mut packet).await.is_err() {
                    break;
                }
                if !sink.emit(packet) {
                    break;
                }
            }
            sink.fail();
        });
        Self {
            input: Control::Pipe(input),
            inbox,
            reader: Some(reader),
            pending: None,
        }
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(crate) fn from_callback(
        control: Arc<dyn Fn(bool) -> io::Result<()> + Send + Sync>,
    ) -> (Self, Arc<InputSink>) {
        let (sink, inbox) = InputSink::channel();
        (
            Self {
                input: Control::Callback(control),
                inbox,
                reader: None,
                pending: None,
            },
            sink,
        )
    }
    pub async fn set_enabled(&mut self, enabled: bool) -> io::Result<()> {
        self.pending = None;
        match &self.input {
            Control::Pipe(pipe) => {
                let mut input = pipe.lock().await;
                input
                    .write_all(&[255, 255, 255, 255, u8::from(enabled)])
                    .await?;
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Control::Callback(apply) => apply(enabled)?,
        }
        // Native ACK is ordered after all events from the previous mode. Drain
        // those events; they must never be delivered under a new input lease.
        loop {
            if let Event::Mode(actual) = self.inbox.next_raw().await? {
                return if actual == enabled {
                    Ok(())
                } else {
                    Err(io::Error::other("native input mode mismatch"))
                };
            }
        }
    }
    pub async fn next(&mut self) -> io::Result<RemoteDesktopInput> {
        if let Some(event) = self.pending.take() {
            return Ok(event);
        }
        loop {
            match self.inbox.next_raw().await? {
                Event::Input(event) => return Ok(event),
                Event::Wheel { x, y, delta } => {
                    // Wheel's position must precede its action even when motion
                    // was coalesced on another channel. Keep the network v2 API.
                    self.pending = Some(RemoteDesktopInput::Wheel { delta });
                    return Ok(RemoteDesktopInput::Move { x, y });
                }
                Event::Mode(_) => {}
            }
        }
    }
}
impl Inbox {
    async fn next_raw(&mut self) -> io::Result<Event> {
        loop {
            if *self.failed.borrow() {
                return Err(io::Error::other(
                    "native input event pipe ended or overflowed",
                ));
            }
            let stamped = tokio::select! {
                biased;
                _ = self.failed.changed() => continue,
                event = self.events.recv() => event,
                changed = self.movement.changed() => {
                    changed.map_err(|_| io::Error::other("native motion pipe ended"))?;
                    self.movement.borrow_and_update().clone()
                }
            };
            if let Some((index, event)) = stamped {
                // Only motion may be superseded. The two channels can become
                // ready between select's polls, so a newer move may be observed
                // just before an older queued key-up or mode ACK. Never discard
                // either of those based on a motion sequence number.
                if !matches!(event, Event::Input(RemoteDesktopInput::Move { .. }))
                    || index > self.consumed
                {
                    self.consumed = self.consumed.max(index);
                    return Ok(event);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn motion_bursts_preserve_releases_and_overflow_fails_closed() {
        let (sink, mut inbox) = InputSink::channel();
        for _ in 0..10000 {
            assert!(sink.emit([1, 1, 0, 2, 0, 0, 0, 0]));
        }
        assert!(sink.emit([4, 0, 0, 0, 0, 65, 0, 0]));
        assert!(sink.emit([6, 0, 0, 0, 0, 0, 0, 0]));
        assert!(matches!(
            inbox.next_raw().await.unwrap(),
            Event::Input(RemoteDesktopInput::Key {
                virtual_key: 65,
                down: false
            })
        ));
        assert!(matches!(
            inbox.next_raw().await.unwrap(),
            Event::Mode(false)
        ));
        // A stalled consumer cannot silently lose a release and remain usable.
        for _ in 0..64 {
            assert!(sink.emit([4, 0, 0, 0, 0, 65, 0, 1]));
        }
        assert!(!sink.emit([4, 0, 0, 0, 0, 65, 0, 0]));
        assert!(inbox.next_raw().await.is_err());
    }

    #[tokio::test]
    async fn motion_channel_race_cannot_discard_key_release_or_mode_barrier() {
        let (events, receive) = mpsc::channel(4);
        let (moves, movement) = watch::channel(None);
        let (_failure, failed) = watch::channel(false);
        let mut inbox = Inbox {
            events: receive,
            movement,
            failed,
            consumed: 0,
        };
        // Reproduce observation order across independent channels: the watch
        // can become ready after select polled the event queue but before it
        // re-polls a key-up that has also arrived in that queue.
        moves.send_replace(Some((
            2,
            Event::Input(RemoteDesktopInput::Move { x: 10, y: 20 }),
        )));
        assert!(matches!(
            inbox.next_raw().await.unwrap(),
            Event::Input(RemoteDesktopInput::Move { .. })
        ));
        events
            .send((
                1,
                Event::Input(RemoteDesktopInput::Key {
                    virtual_key: 162,
                    down: false,
                }),
            ))
            .await
            .unwrap();
        let release = tokio::time::timeout(std::time::Duration::from_secs(1), inbox.next_raw())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            release,
            Event::Input(RemoteDesktopInput::Key { down: false, .. })
        ));

        moves.send_replace(Some((
            4,
            Event::Input(RemoteDesktopInput::Move { x: 30, y: 40 }),
        )));
        let _ = inbox.next_raw().await.unwrap();
        events.send((3, Event::Mode(false))).await.unwrap();
        let barrier = tokio::time::timeout(std::time::Duration::from_secs(1), inbox.next_raw())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(barrier, Event::Mode(false)));
    }
}
fn decode(packet: [u8; 8]) -> Option<Event> {
    if packet[7] > 1 {
        return None;
    }
    let x = u16::from_le_bytes([packet[1], packet[2]]);
    let y = u16::from_le_bytes([packet[3], packet[4]]);
    let value = u16::from_le_bytes([packet[5], packet[6]]);
    let down = packet[7] != 0;
    if packet[0] == 6 {
        return (value <= 1).then_some(Event::Mode(value == 1));
    }
    if packet[0] == 3 {
        return Some(Event::Wheel {
            x,
            y,
            delta: value as i16,
        });
    }
    let event = match packet[0] {
        1 => RemoteDesktopInput::Move { x, y },
        2 => RemoteDesktopInput::Button {
            x,
            y,
            button: match value {
                0 => DesktopMouseButton::Left,
                1 => DesktopMouseButton::Right,
                2 => DesktopMouseButton::Middle,
                _ => return None,
            },
            down,
        },
        4 => RemoteDesktopInput::Key {
            virtual_key: value,
            down,
        },
        5 => RemoteDesktopInput::ReleaseAll,
        _ => return None,
    };
    event.validate().ok()?;
    Some(Event::Input(event))
}
