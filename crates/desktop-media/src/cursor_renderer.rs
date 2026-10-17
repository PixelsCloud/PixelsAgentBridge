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
        let packet = encode(&self.config, update);
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
fn encode(config: &DesktopVideoConfig, update: &CursorUpdate) -> Vec<u8> {
    let mut packet = Vec::with_capacity(44 + update.bytes.len());
    packet.push(1);
    packet.push(u8::from(update.header.position.is_some()));
    let (x, y) = update
        .header
        .position
        .map(|point| {
            (
                i64::from(point.x) - i64::from(config.screen.x),
                i64::from(point.y) - i64::from(config.screen.y),
            )
        })
        .unwrap_or((0, 0));
    packet.extend_from_slice(&x.to_le_bytes());
    packet.extend_from_slice(&y.to_le_bytes());
    packet.extend_from_slice(&config.screen.desktop_width.to_le_bytes());
    packet.extend_from_slice(&config.screen.desktop_height.to_le_bytes());
    match &update.header.image {
        None => packet.push(0),
        Some(image) => {
            packet.push(match image.format {
                DesktopCursorFormat::PremultipliedBgra => 1,
                DesktopCursorFormat::AndXorBgra => 2,
            });
            for value in [image.width, image.height, image.hotspot_x, image.hotspot_y] {
                packet.extend_from_slice(&value.to_le_bytes());
            }
            packet.extend_from_slice(&update.bytes);
        }
    }
    packet
}
