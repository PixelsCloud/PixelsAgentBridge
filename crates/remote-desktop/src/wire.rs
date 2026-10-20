use crate::{DesktopError, Result};
use pab_protocol::*;
use pab_transport::{PabFrameReader, PabFrameWriter};
use std::time::Duration;

// Deliberately no Debug for video bytes: tracing errors must not archive frames.
pub struct VideoFrame {
    pub header: DesktopFrameHeader,
    pub bytes: Vec<u8>,
}

pub struct DesktopVideoReader {
    reader: Option<PabFrameReader>,
    config: DesktopVideoConfig,
    next_frame: u64,
}
pub struct DesktopVideoWriter {
    writer: Option<PabFrameWriter>,
    config: DesktopVideoConfig,
    next_frame: u64,
}
impl DesktopVideoReader {
    /// Static desktops may produce no frame for hours. After the first byte,
    /// partial headers and payloads still have deadlines. On cancellation this
    /// generation is poisoned, just like `receive`.
    pub async fn receive_after_idle(&mut self, timeout: Duration) -> Result<VideoFrame> {
        let mut reader = self.reader.take().ok_or(DesktopError::Closed)?;
        let next = self
            .next_frame
            .checked_add(1)
            .ok_or(DesktopError::Protocol("frame sequence overflow"))?;
        let header: DesktopFrameHeader = serde_json::from_slice(
            &reader
                .read_after_idle(DESKTOP_CONTROL_BYTES, timeout)
                .await?,
        )?;
        header
            .validate(&self.config, self.next_frame)
            .map_err(DesktopError::Protocol)?;
        let size = header.payload_bytes as usize;
        let bytes = tokio::time::timeout(timeout, async {
            let mut bytes = Vec::with_capacity(size);
            while bytes.len() < size {
                let chunk = reader
                    .read(DESKTOP_VIDEO_CHUNK_BYTES.min(size - bytes.len()))
                    .await?;
                bytes.extend_from_slice(&chunk);
            }
            Ok::<_, DesktopError>(bytes)
        })
        .await
        .map_err(|_| DesktopError::Timeout)??;
        self.reader = Some(reader);
        self.next_frame = next;
        Ok(VideoFrame { header, bytes })
    }
    pub fn new(reader: PabFrameReader, config: DesktopVideoConfig) -> Result<Self> {
        config.validate().map_err(DesktopError::Protocol)?;
        Ok(Self {
            reader: Some(reader),
            config,
            next_frame: 1,
        })
    }
    /// Cancellation-safe at the session boundary: if this future is dropped or
    /// times out midway through a header/payload, the stream is DROPPED and this
    /// reader remains poisoned. Never interpret remaining payload as a new header.
    pub async fn receive(&mut self, timeout: Duration) -> Result<VideoFrame> {
        let mut reader = self.reader.take().ok_or(DesktopError::Closed)?;
        let next = self
            .next_frame
            .checked_add(1)
            .ok_or(DesktopError::Protocol("frame sequence overflow"))?;
        let frame = tokio::time::timeout(timeout, async {
            let header: DesktopFrameHeader =
                serde_json::from_slice(&reader.read(DESKTOP_CONTROL_BYTES).await?)?;
            header
                .validate(&self.config, self.next_frame)
                .map_err(DesktopError::Protocol)?;
            let size = header.payload_bytes as usize;
            let mut bytes = Vec::with_capacity(size);
            while bytes.len() < size {
                let chunk = reader
                    .read(DESKTOP_VIDEO_CHUNK_BYTES.min(size - bytes.len()))
                    .await?;
                bytes.extend_from_slice(&chunk);
            }
            Ok::<_, DesktopError>(VideoFrame { header, bytes })
        })
        .await
        .map_err(|_| DesktopError::Timeout)??;
        self.reader = Some(reader);
        self.next_frame = next;
        Ok(frame)
    }
}
impl DesktopVideoWriter {
    pub fn new(writer: PabFrameWriter, config: DesktopVideoConfig) -> Result<Self> {
        config.validate().map_err(DesktopError::Protocol)?;
        Ok(Self {
            writer: Some(writer),
            config,
            next_frame: 1,
        })
    }
    /// On error or cancellation the writer cannot be reused; restart the video
    /// generation instead of dropping arbitrary H.264 reference frames.
    pub async fn send(&mut self, frame: &VideoFrame, timeout: Duration) -> Result<()> {
        frame
            .header
            .validate(&self.config, self.next_frame)
            .map_err(DesktopError::Protocol)?;
        if frame.bytes.len() != frame.header.payload_bytes as usize {
            return Err(DesktopError::Protocol("frame payload size mismatch"));
        }
        let next = self
            .next_frame
            .checked_add(1)
            .ok_or(DesktopError::Protocol("frame sequence overflow"))?;
        let mut writer = self.writer.take().ok_or(DesktopError::Closed)?;
        tokio::time::timeout(timeout, async {
            writer
                .write(&serde_json::to_vec(&frame.header)?, DESKTOP_CONTROL_BYTES)
                .await?;
            for chunk in frame.bytes.chunks(DESKTOP_VIDEO_CHUNK_BYTES) {
                writer.write(chunk, DESKTOP_VIDEO_CHUNK_BYTES).await?;
            }
            Ok::<_, DesktopError>(())
        })
        .await
        .map_err(|_| DesktopError::Timeout)??;
        self.writer = Some(writer);
        self.next_frame = next;
        Ok(())
    }
}
