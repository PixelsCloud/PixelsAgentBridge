//! A generation-owned cursor submitter independent of video submission.
//! The pipe mutex prevents interleaved commands; any failed/cancelled submission
//! retires this handle. The owner must retire its viewer after a local IO error.
use crate::renderer::CursorAcknowledgements;
use pab_protocol::*;
use pab_remote_desktop::CursorUpdate;
use std::sync::{Arc, Weak};
use tokio::{io::AsyncWriteExt, process::ChildStdin, sync::Mutex};
type Error = Box<dyn std::error::Error + Send + Sync>;

pub struct NativeCursorRenderer {
    pipe: Option<Weak<Mutex<ChildStdin>>>,
    config: DesktopVideoConfig,
    next_sequence: u64,
    image_id: u64,
    acknowledgements: CursorAcknowledgements,
}
impl NativeCursorRenderer {
    /// Local chrome command; does not affect the media generation or remote input lease.
    pub async fn fullscreen(&mut self, enabled: bool) -> Result<(), Error> {
        self.send(vec![3, u8::from(enabled)]).await
    }
    /// Local overlay data shares the serialized native UI lane, never the wire.
    pub async fn overlay(&mut self, text: &str) -> Result<(), Error> {
        if text.len() > 2048 {
            return Err("viewer statistics too large".into());
        }
        self.send([&[2][..], text.as_bytes()].concat()).await
    }
    pub(crate) fn new(
        pipe: &Arc<Mutex<ChildStdin>>,
        config: DesktopVideoConfig,
        acknowledgements: CursorAcknowledgements,
    ) -> Self {
        Self {
            pipe: Some(Arc::downgrade(pipe)),
            config,
            next_sequence: 1,
            image_id: 0,
            acknowledgements,
        }
    }
    pub async fn submit(&mut self, update: &CursorUpdate) -> Result<(), Error> {
        self.next_sequence
            .checked_add(1)
            .ok_or("cursor sequence exhausted")?;
        if update
            .header
            .validate(&self.config, self.next_sequence, self.image_id)?
            != update.bytes.len()
        {
            return Err("cursor payload length mismatch".into());
        }
        let packet = crate::cursor_packet::encode(&self.config, update);
        self.send(packet).await?;
        self.next_sequence += 1;
        self.image_id = update.header.image_id;
        Ok(())
    }
    /// A source failure hides the last cursor without closing video. Local
    /// renderer/pipe failures instead require the owner to retire the viewer.
    pub async fn clear(&mut self) -> Result<(), Error> {
        self.send(vec![0]).await
    }
    async fn send(&mut self, packet: Vec<u8>) -> Result<(), Error> {
        let pipe = self.pipe.take().ok_or("cursor renderer ended")?;
        let input = pipe.upgrade().ok_or("native viewer closed")?;
        let sequence = self.acknowledgements.prepare()?;
        {
            let mut input = input.lock().await;
            input.write_u32_le(u32::MAX - 1).await?;
            input.write_u32_le((packet.len() + 8).try_into()?).await?;
            input.write_u64_le(sequence).await?;
            input.write_all(&packet).await?;
        }
        self.acknowledgements.consumed(sequence).await?;
        self.pipe = Some(pipe);
        Ok(())
    }
}
