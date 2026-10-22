use super::*;

#[derive(Clone)]
pub struct RemoteDesktopClipboardControl {
    connection: PabConnection,
    config: DesktopVideoConfig,
    ended: watch::Receiver<bool>,
    supported: bool,
}
impl RemoteDesktopSession {
    pub fn clipboard_control(&self) -> RemoteDesktopClipboardControl {
        RemoteDesktopClipboardControl { connection: self.connection.clone(), config: self.opened.config.clone(), ended: self.generation_ended.subscribe(), supported: self.clipboard_version == Some(DESKTOP_CLIPBOARD_VERSION) }
    }
}
impl RemoteDesktopClipboardControl {
    pub fn is_supported(&self) -> bool { self.supported }
    pub async fn exchange(&mut self, operation: DesktopClipboardOperation) -> Result<DesktopClipboardSnapshot> {
        if !self.supported { return Err(DesktopError::reject(DesktopErrorCode::Unsupported, "remote desktop text clipboard requires an updated remote client")); }
        operation.validate().map_err(DesktopError::Protocol)?;
        if *self.ended.borrow() || self.ended.has_changed().is_err() { return Err(DesktopError::Closed); }
        let timeout = Duration::from_secs(4);
        let request = match &operation {
            DesktopClipboardOperation::Read { after } => DesktopClipboardRequest::Read { after: *after },
            DesktopClipboardOperation::Write { text } => DesktopClipboardRequest::Write { bytes: text.len() as u32 },
        };
        let work = async {
            let mut stream = stream(&self.connection, RemoteDesktopRequest::Clipboard { version: DESKTOP_CLIPBOARD_VERSION,
                session_id: self.config.session_id, generation: self.config.generation, request }, timeout).await?;
            if let DesktopClipboardOperation::Write { text } = operation {
                if !matches!(response(&mut stream, timeout).await?, RemoteDesktopReply::Ready) { return Err(DesktopError::Protocol("unexpected clipboard write reply")); }
                let (mut writer, reader) = stream.into_framed();
                for chunk in text.as_bytes().chunks(DESKTOP_CLIPBOARD_CHUNK_BYTES) { writer.write(chunk, DESKTOP_CLIPBOARD_CHUNK_BYTES).await?; }
                return read_snapshot(reader, timeout).await;
            }
            let (_, reader) = stream.into_framed();
            read_snapshot(reader, timeout).await
        };
        tokio::select! {
            biased;
            _ = self.ended.changed() => Err(DesktopError::Closed),
            result = tokio::time::timeout(timeout, work) => result.map_err(|_| DesktopError::Timeout)?,
        }
    }
}
async fn read_snapshot(mut reader: pab_transport::PabFrameReader, timeout: Duration) -> Result<DesktopClipboardSnapshot> {
    let bytes = tokio::time::timeout(timeout, reader.read(DESKTOP_CONTROL_BYTES)).await.map_err(|_| DesktopError::Timeout)??;
    let reply: DeviceTaskResponse = serde_json::from_slice(&bytes)?;
    let (revision, changed, size) = match reply {
        DeviceTaskResponse::RemoteDesktop { reply: RemoteDesktopReply::Clipboard { version: DESKTOP_CLIPBOARD_VERSION, revision, changed, bytes } } => (revision, changed, bytes),
        DeviceTaskResponse::RemoteDesktop { reply: RemoteDesktopReply::Error { code, message } } => return Err(DesktopError::reject(code, message)),
        _ => return Err(DesktopError::Protocol("unexpected clipboard reply")),
    };
    if revision == 0 || size.is_some_and(|size| size as usize > DESKTOP_CLIPBOARD_MAX_BYTES) || (!changed && size.is_some()) {
        return Err(DesktopError::Protocol("invalid clipboard reply"));
    }
    let text = if let Some(size) = size {
        let mut text = Vec::with_capacity(size as usize);
        while text.len() < size as usize { text.extend_from_slice(&reader.read(DESKTOP_CLIPBOARD_CHUNK_BYTES.min(size as usize - text.len())).await?); }
        let text = String::from_utf8(text).map_err(|_| DesktopError::Protocol("clipboard text is not UTF-8"))?;
        validate_clipboard_text(&text).map_err(DesktopError::Protocol)?;
        Some(text)
    } else { None };
    Ok(DesktopClipboardSnapshot { revision, changed, text })
}
