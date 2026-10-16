//! Decoder consumption and presentation are different events. Keep metadata
//! for at most the negotiated in-flight window; never acknowledge mere receipt.
use pab_protocol::{DESKTOP_MAX_IN_FLIGHT, DesktopFrameHeader};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::watch,
};

type Error = Box<dyn std::error::Error + Send + Sync>;
#[derive(Clone, Default)]
struct Progress {
    decoder: Option<&'static str>,
    consumed: u64,
    presented: Option<DesktopFrameHeader>,
    summary: Option<serde_json::Value>,
    error: Option<String>,
}
pub(crate) struct Renderer {
    pending: Arc<Mutex<VecDeque<DesktopFrameHeader>>>,
    progress: watch::Receiver<Progress>,
    task: tokio::task::JoinHandle<()>,
}
/// Independent output notification, including when no new desktop frame arrives.
pub struct NativePresentations {
    progress: watch::Receiver<Progress>,
    last: u64,
}
impl NativePresentations {
    pub async fn next(&mut self) -> Result<DesktopFrameHeader, Error> {
        loop {
            let value = self.progress.borrow_and_update().clone();
            if let Some(error) = value.error {
                return Err(error.into());
            }
            if let Some(header) = value.presented.filter(|h| h.frame_id > self.last) {
                self.last = header.frame_id;
                return Ok(header);
            }
            if value.summary.is_some() {
                return Err("native renderer ended".into());
            }
            self.progress
                .changed()
                .await
                .map_err(|_| "native renderer output closed")?;
        }
    }
}
impl Renderer {
    pub(crate) fn start(output: impl AsyncRead + Unpin + Send + 'static) -> Self {
        let pending = Arc::new(Mutex::new(VecDeque::new()));
        let (sender, progress) = watch::channel(Progress::default());
        let queue = pending.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = receive(output, queue, &sender).await {
                sender.send_modify(|state| state.error = Some(error.to_string()));
            }
        });
        Self {
            pending,
            progress,
            task,
        }
    }
    pub(crate) fn presentations(&self) -> NativePresentations {
        NativePresentations {
            progress: self.progress.clone(),
            last: 0,
        }
    }
    pub(crate) async fn enqueue(&self, header: DesktopFrameHeader) -> Result<(), Error> {
        let mut updates = self.progress.clone();
        loop {
            let state = updates.borrow_and_update().clone();
            if let Some(error) = state.error {
                return Err(error.into());
            }
            if state.summary.is_some() {
                return Err("native renderer already finished".into());
            }
            {
                let mut pending = self.pending.lock().unwrap();
                if pending.len() < DESKTOP_MAX_IN_FLIGHT as usize {
                    pending.push_back(header);
                    return Ok(());
                }
            }
            updates
                .changed()
                .await
                .map_err(|_| "native renderer output closed")?;
        }
    }
    pub(crate) async fn consumed(&self, frame: u64) -> Result<(), Error> {
        let mut updates = self.progress.clone();
        loop {
            let state = updates.borrow_and_update().clone();
            if let Some(error) = state.error {
                return Err(error.into());
            }
            if state.consumed >= frame {
                return Ok(());
            }
            updates
                .changed()
                .await
                .map_err(|_| "native renderer output closed")?;
        }
    }
    pub(crate) async fn summary(&self) -> Result<serde_json::Value, Error> {
        let mut updates = self.progress.clone();
        loop {
            let state = updates.borrow_and_update().clone();
            if let Some(error) = state.error {
                return Err(error.into());
            }
            if let Some(summary) = state.summary {
                return Ok(summary);
            }
            updates
                .changed()
                .await
                .map_err(|_| "native renderer output closed")?;
        }
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn receive(
    mut output: impl AsyncRead + Unpin,
    pending: Arc<Mutex<VecDeque<DesktopFrameHeader>>>,
    sender: &watch::Sender<Progress>,
) -> Result<(), Error> {
    let mut state = Progress::default();
    loop {
        match output.read_u8().await? {
            1 => {
                let index = output.read_u64_le().await?;
                let frame_id = index.checked_add(1).ok_or("decoder sequence exhausted")?;
                if index != state.consumed
                    || !pending
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|h| h.frame_id == frame_id)
                {
                    return Err("native decoder consumed an unsubmitted frame".into());
                }
                state.consumed = frame_id;
            }
            2 => {
                let index = output.read_u64_le().await?;
                let header = pending
                    .lock()
                    .unwrap()
                    .pop_front()
                    .ok_or("native renderer has no pending frame")?;
                if index.checked_add(1) != Some(header.frame_id) || header.frame_id > state.consumed
                {
                    return Err("native decoder reordered or invented a frame".into());
                }
                state.presented = Some(header);
            }
            3 => {
                let displayed = output.read_u64_le().await?;
                let pixel_checks = output.read_u64_le().await?;
                let max_rgb_error = output.read_u32_le().await?;
                if !pending.lock().unwrap().is_empty() || displayed > state.consumed {
                    return Err("native decoder did not drain submitted frames".into());
                }
                state.summary = Some(
                    serde_json::json!({"displayed":displayed,"test_pixel_checks":pixel_checks,"test_max_rgb_error":max_rgb_error,"decoder":state.decoder}),
                );
                sender.send_replace(state);
                return Ok(());
            }
            4 => {
                if state.decoder.is_some() || state.consumed != 0 {
                    return Err("native decoder path changed within generation".into());
                }
                state.decoder = Some(match output.read_u8().await? {
                    1 => "d3d11_hardware",
                    2 => "openh264_software",
                    _ => return Err("invalid native decoder path".into()),
                });
            }
            _ => return Err("invalid native renderer event".into()),
        }
        sender.send_replace(state.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_protocol::{REMOTE_DESKTOP_SCHEMA_VERSION, RequestId};
    use std::time::Duration;
    use tokio::{
        io::{AsyncWriteExt, DuplexStream},
        time::timeout,
    };

    fn header(frame_id: u64) -> DesktopFrameHeader {
        DesktopFrameHeader {
            version: REMOTE_DESKTOP_SCHEMA_VERSION,
            session_id: RequestId::new(),
            generation: 1,
            frame_id,
            capture_time_us: frame_id * 1000,
            keyframe: frame_id == 1,
            payload_bytes: 16,
        }
    }
    async fn event(pipe: &mut DuplexStream, kind: u8, index: u64) {
        pipe.write_u8(kind).await.unwrap();
        pipe.write_u64_le(index).await.unwrap();
    }

    #[tokio::test]
    async fn delayed_decoder_output_is_bounded_and_does_not_grant_presentation_credit() {
        let (mut pipe, output) = tokio::io::duplex(256);
        let renderer = Renderer::start(output);
        let mut presentations = renderer.presentations();
        for frame in 1..=DESKTOP_MAX_IN_FLIGHT {
            renderer.enqueue(header(frame)).await.unwrap();
            event(&mut pipe, 1, frame - 1).await;
            timeout(Duration::from_secs(1), renderer.consumed(frame))
                .await
                .unwrap()
                .unwrap();
        }
        assert!(
            timeout(Duration::from_millis(20), presentations.next())
                .await
                .is_err()
        );
        assert!(
            timeout(Duration::from_millis(20), renderer.enqueue(header(5)))
                .await
                .is_err()
        );
        // There is no new compressed packet here. Output must still wake the
        // independent presentation reader and unblock the bounded input queue.
        event(&mut pipe, 2, 0).await;
        assert_eq!(
            timeout(Duration::from_secs(1), presentations.next())
                .await
                .unwrap()
                .unwrap()
                .frame_id,
            1
        );
        renderer.enqueue(header(5)).await.unwrap();
        event(&mut pipe, 1, 4).await;
        for index in 1..5 {
            event(&mut pipe, 2, index).await;
        }
        // Physical-display statistics may be lower than decoded/rendered count.
        event(&mut pipe, 3, 4).await;
        pipe.write_u64_le(0).await.unwrap();
        pipe.write_u32_le(0).await.unwrap();
        assert_eq!(
            timeout(Duration::from_secs(1), renderer.summary())
                .await
                .unwrap()
                .unwrap()["displayed"],
            4
        );
        assert_eq!(presentations.next().await.unwrap().frame_id, 5);
        assert!(presentations.next().await.is_err());
    }

    #[tokio::test]
    async fn unexpected_or_unconsumed_output_and_incomplete_drain_fail() {
        for kind in [1, 2, 3] {
            let (mut pipe, output) = tokio::io::duplex(256);
            let renderer = Renderer::start(output);
            renderer.enqueue(header(1)).await.unwrap();
            // Invented consumption; presentation before consumption; or final
            // summary with an undrained packet must never look like success.
            event(&mut pipe, kind, if kind == 1 { u64::MAX } else { 0 }).await;
            if kind == 3 {
                pipe.write_u64_le(0).await.unwrap();
                pipe.write_u32_le(0).await.unwrap();
            }
            assert!(
                timeout(Duration::from_secs(1), renderer.summary())
                    .await
                    .unwrap()
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn truncated_worker_event_wakes_pending_submission_and_output() {
        let (mut pipe, output) = tokio::io::duplex(256);
        let renderer = Renderer::start(output);
        let mut presentations = renderer.presentations();
        renderer.enqueue(header(1)).await.unwrap();
        pipe.write_u8(1).await.unwrap();
        pipe.write_u32_le(0).await.unwrap();
        drop(pipe);
        assert!(
            timeout(Duration::from_secs(1), renderer.consumed(1))
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            timeout(Duration::from_secs(1), presentations.next())
                .await
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn decoder_path_is_reported_and_cannot_change_within_generation() {
        let (mut pipe, output) = tokio::io::duplex(256);
        let renderer = Renderer::start(output);
        pipe.write_all(&[4, 2]).await.unwrap();
        event(&mut pipe, 3, 0).await;
        pipe.write_u64_le(0).await.unwrap();
        pipe.write_u32_le(0).await.unwrap();
        assert_eq!(
            renderer.summary().await.unwrap()["decoder"],
            "openh264_software"
        );
        for bytes in [&[4, 2, 4, 1][..], &[4, 9][..]] {
            let (mut pipe, output) = tokio::io::duplex(256);
            let renderer = Renderer::start(output);
            pipe.write_all(bytes).await.unwrap();
            assert!(
                timeout(Duration::from_secs(1), renderer.summary())
                    .await
                    .unwrap()
                    .is_err()
            );
        }
    }
}
