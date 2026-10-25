//! Measurement must never put filesystem/console backpressure on video ACKs.
use serde_json::Value;
use std::{
    io::{self, BufWriter, Write},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

enum Message {
    Event(Value),
    Finish,
}
struct Sender {
    channel: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
}
static SENDER: OnceLock<Sender> = OnceLock::new();
pub struct Telemetry {
    channel: SyncSender<Message>,
    writer: thread::JoinHandle<io::Result<u64>>,
}

pub fn emit(value: Value) {
    if let Some(sender) = SENDER.get() {
        if sender.channel.try_send(Message::Event(value)).is_err() {
            sender.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
impl Telemetry {
    pub fn start() -> Self {
        let (channel, receiver) = mpsc::sync_channel(4096);
        let dropped = Arc::new(AtomicU64::new(0));
        assert!(
            SENDER
                .set(Sender {
                    channel: channel.clone(),
                    dropped: dropped.clone()
                })
                .is_ok()
        );
        let writer = thread::spawn(move || {
            let stdout = io::stdout();
            write_events(stdout.lock(), receiver, dropped)
        });
        Self { channel, writer }
    }
    pub fn finish(self) -> io::Result<u64> {
        // Only after the runtime and owned desktop connection have stopped.
        let _ = self.channel.send(Message::Finish);
        self.writer
            .join()
            .map_err(|_| io::Error::other("telemetry writer panicked"))?
    }
}
fn write_events(
    output: impl Write,
    receiver: mpsc::Receiver<Message>,
    dropped: Arc<AtomicU64>,
) -> io::Result<u64> {
    let mut output = BufWriter::with_capacity(64 * 1024, output);
    let mut flushed = Instant::now();
    let mut count = 0u64;
    while let Ok(message) = receiver.recv() {
        match message {
            Message::Event(value) => {
                serde_json::to_writer(&mut output, &value)?;
                output.write_all(b"\n")?;
                count += 1;
            }
            Message::Finish => break,
        }
        if flushed.elapsed() >= Duration::from_secs(1) {
            output.flush()?;
            flushed = Instant::now();
        }
    }
    let lost = dropped.load(Ordering::Relaxed);
    if lost != 0 {
        writeln!(
            output,
            "{}",
            serde_json::json!({"event":"error","message":"measurement telemetry overflow","dropped_events":lost})
        )?;
    }
    writeln!(
        output,
        "{}",
        serde_json::json!({"event":"telemetry","written_events":count,"dropped_events":lost})
    )?;
    output.flush()?;
    Ok(lost)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocked_output_does_not_block_video_observations_and_overflow_is_explicit() {
        let (channel, receiver) = mpsc::sync_channel(8);
        let dropped = Arc::new(AtomicU64::new(0));
        // Deliberately do not service the output queue while video progresses.
        let began = Instant::now();
        for frame in 0..100 {
            if channel
                .try_send(Message::Event(
                    serde_json::json!({"event":"presented","frame":frame}),
                ))
                .is_err()
            {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        assert!(began.elapsed() < Duration::from_secs(1));
        drop(channel);
        let mut output = Vec::new();
        assert_eq!(write_events(&mut output, receiver, dropped).unwrap(), 92);
        let rows: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(
            rows.iter()
                .any(|row| row["event"] == "error" && row["dropped_events"] == 92)
        );
    }
}
