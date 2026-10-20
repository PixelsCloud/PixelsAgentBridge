//! Cursor framing on an existing reliable stream, independent of video credit.
//! The session owner must bind/authorize the stream before constructing these
//! adapters. They do not open connections, claim a viewer or bypass auth.
use crate::{DesktopError, Result};
use pab_protocol::*;
use pab_transport::{PabFrameReader, PabFrameWriter};
use std::time::Duration;

/// No Debug: cursor images can be app-defined and must not enter logs.
pub struct CursorUpdate {
    pub header: DesktopCursorHeader,
    pub bytes: Vec<u8>,
}

struct State {
    config: DesktopVideoConfig,
    sequence: u64,
    image_id: u64,
}
impl State {
    fn new(config: DesktopVideoConfig) -> Result<Self> {
        config.validate().map_err(DesktopError::Protocol)?;
        Ok(Self {
            config,
            sequence: 1,
            image_id: 0,
        })
    }
    fn validate(&self, header: &DesktopCursorHeader) -> Result<usize> {
        self.sequence
            .checked_add(1)
            .ok_or(DesktopError::Protocol("cursor sequence exhausted"))?;
        header
            .validate(&self.config, self.sequence, self.image_id)
            .map_err(DesktopError::Protocol)
    }
    fn commit(&mut self, header: &DesktopCursorHeader) {
        // validate() checked overflow before any stream IO or allocation.
        self.sequence += 1;
        self.image_id = header.image_id;
    }
}

pub struct DesktopCursorReader {
    reader: Option<PabFrameReader>,
    state: State,
}
impl DesktopCursorReader {
    pub fn new(reader: PabFrameReader, config: DesktopVideoConfig) -> Result<Self> {
        Ok(Self {
            reader: Some(reader),
            state: State::new(config)?,
        })
    }
    /// A stationary/hidden cursor can stay idle indefinitely. Once a header
    /// starts, both framing and the complete payload have finite deadlines.
    /// Dropping this pending future poisons ONLY this cursor stream, even if
    /// idle: never reuse an ambiguous partial read after cancellation.
    pub async fn receive_after_idle(&mut self, timeout: Duration) -> Result<CursorUpdate> {
        if timeout.is_zero() {
            return Err(DesktopError::Protocol("cursor timeout must be positive"));
        }
        let mut reader = self.reader.take().ok_or(DesktopError::Closed)?;
        let header: DesktopCursorHeader = serde_json::from_slice(
            &reader
                .read_after_idle(DESKTOP_CONTROL_BYTES, timeout)
                .await?,
        )?;
        let size = self.state.validate(&header)?;
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
        self.state.commit(&header);
        self.reader = Some(reader);
        Ok(CursorUpdate { header, bytes })
    }
}

pub struct DesktopCursorWriter {
    writer: Option<PabFrameWriter>,
    state: State,
}
impl DesktopCursorWriter {
    pub fn new(writer: PabFrameWriter, config: DesktopVideoConfig) -> Result<Self> {
        Ok(Self {
            writer: Some(writer),
            state: State::new(config)?,
        })
    }
    /// No queued images or position history. The producer should keep its
    /// latest snapshot while this bounded write is pending. Failed/cancelled
    /// writes retire this stream; they are never replayed on it.
    pub async fn send(&mut self, update: &CursorUpdate, timeout: Duration) -> Result<()> {
        if timeout.is_zero() {
            return Err(DesktopError::Protocol("cursor timeout must be positive"));
        }
        let size = self.state.validate(&update.header)?;
        if size != update.bytes.len() {
            return Err(DesktopError::Protocol(
                "cursor image payload length mismatch",
            ));
        }
        let mut writer = self.writer.take().ok_or(DesktopError::Closed)?;
        tokio::time::timeout(timeout, async {
            writer
                .write(&serde_json::to_vec(&update.header)?, DESKTOP_CONTROL_BYTES)
                .await?;
            for chunk in update.bytes.chunks(DESKTOP_VIDEO_CHUNK_BYTES) {
                writer.write(chunk, DESKTOP_VIDEO_CHUNK_BYTES).await?;
            }
            Ok::<_, DesktopError>(())
        })
        .await
        .map_err(|_| DesktopError::Timeout)??;
        self.state.commit(&update.header);
        self.writer = Some(writer);
        Ok(())
    }
}
