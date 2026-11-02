use crate::{DesktopError, Result};
use pab_protocol::*;
use pab_transport::{MAX_BINARY_FRAME_BYTES, PabFrameReader, PabFrameWriter};
pub(super) async fn read(
    reader: &mut PabFrameReader,
    length: u32,
    maximum: usize,
) -> Result<Vec<u8>> {
    if length as usize > maximum {
        return Err(DesktopError::Protocol("file_clipboard_frame_too_large"));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    while bytes.len() < length as usize {
        let chunk = reader
            .read((length as usize - bytes.len()).min(MAX_BINARY_FRAME_BYTES))
            .await?;
        if chunk.is_empty() {
            return Err(DesktopError::Protocol("file_clipboard_empty_chunk"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
pub(super) async fn write(writer: &mut PabFrameWriter, bytes: &[u8]) -> Result<()> {
    for chunk in bytes.chunks(MAX_BINARY_FRAME_BYTES) {
        writer.write(chunk, MAX_BINARY_FRAME_BYTES).await?;
    }
    Ok(())
}
pub(super) fn verify(snapshot: &FileClipboardSnapshot) -> Result<()> {
    use sha2::{Digest, Sha256};
    snapshot.validate().map_err(DesktopError::Protocol)?;
    if snapshot.data.len() != snapshot.bytes as usize
        || snapshot.bytes > 0
            && snapshot.sha256.as_deref()
                != Some(format!("{:x}", Sha256::digest(&snapshot.data)).as_str())
    {
        return Err(DesktopError::Protocol("file_clipboard_invalid_digest"));
    }
    Ok(())
}
