use super::*;

impl DesktopHost {
    pub(super) async fn serve_clipboard(&self, owner: OperatorRef, account: Option<UserId>, version: u16,
        session_id: RequestId, generation: u64, request: DesktopClipboardRequest, mut stream: PabBiStream, timeout: Duration) -> Result<()> {
        let checked = self.entry(owner, account, session_id, false).and_then(|entry| {
            if version != DESKTOP_CLIPBOARD_VERSION { return Err(DesktopError::reject(DesktopErrorCode::Unsupported, "clipboard version is unavailable")); }
            let video = entry.video(generation)?;
            if !entry.control_attached.load(Ordering::Acquire) || *video.cancelled.borrow() || !video.frame_ready.load(Ordering::Acquire) {
                return Err(DesktopError::Closed);
            }
            Ok((entry, video))
        });
        let (entry, video) = match checked { Ok(value) => value, Err(error) => return rejected(&mut stream, error, timeout).await };
        let backend = self.shared.backend.as_ref().ok_or(DesktopError::Closed)?;
        // A failed clipboard exchange owns only this stream, never the video/input lease.
        let operation = match request {
            DesktopClipboardRequest::Read { after } => DesktopClipboardOperation::Read { after },
            DesktopClipboardRequest::Write { bytes } => {
                if bytes as usize > DESKTOP_CLIPBOARD_MAX_BYTES { return rejected(&mut stream, DesktopError::Protocol("clipboard text exceeds 1 MiB"), timeout).await; }
                reply(&mut stream, RemoteDesktopReply::Ready, timeout).await?;
                let (writer, mut reader) = stream.into_framed();
                let work = async {
                    let mut text = Vec::with_capacity(bytes as usize);
                    while text.len() < bytes as usize {
                        text.extend_from_slice(&reader.read(DESKTOP_CLIPBOARD_CHUNK_BYTES.min(bytes as usize - text.len())).await?);
                    }
                    let text = String::from_utf8(text).map_err(|_| DesktopError::Protocol("clipboard text is not UTF-8"))?;
                    validate_clipboard_text(&text).map_err(DesktopError::Protocol)?;
                    backend.clipboard(video.config.clone(), DesktopClipboardOperation::Write { text }).await
                };
                let result = tokio::select! {
                    biased;
                    _ = cancelled(&entry.cancelled) => Err(DesktopError::Closed),
                    _ = cancelled(&video.cancelled) => Err(DesktopError::Closed),
                    result = tokio::time::timeout(timeout, work) => result.unwrap_or(Err(DesktopError::Timeout)),
                };
                return send_snapshot(writer, result, timeout).await;
            }
        };
        let result = tokio::select! {
            biased;
            _ = cancelled(&entry.cancelled) => Err(DesktopError::Closed),
            _ = cancelled(&video.cancelled) => Err(DesktopError::Closed),
            result = tokio::time::timeout(timeout, backend.clipboard(video.config.clone(), operation)) => result.unwrap_or(Err(DesktopError::Timeout)),
        };
        let (writer, _) = stream.into_framed();
        send_snapshot(writer, result, timeout).await
    }
}

async fn send_snapshot(mut writer: pab_transport::PabFrameWriter, result: Result<DesktopClipboardSnapshot>, timeout: Duration) -> Result<()> {
    let result = result.and_then(|snapshot| { snapshot.validate().map_err(DesktopError::Protocol)?; Ok(snapshot) });
    let (header, text) = match result {
        Ok(snapshot) => (RemoteDesktopReply::Clipboard { version: DESKTOP_CLIPBOARD_VERSION, revision: snapshot.revision,
            changed: snapshot.changed, bytes: snapshot.text.as_ref().map(|text| text.len() as u32) }, snapshot.text),
        Err(error) => (RemoteDesktopReply::Error { code: error.code(), message: error.to_string() }, None),
    };
    tokio::time::timeout(timeout, async {
        writer.write(&serde_json::to_vec(&DeviceTaskResponse::RemoteDesktop { reply: header })?, DESKTOP_CONTROL_BYTES).await?;
        if let Some(text) = text { for chunk in text.as_bytes().chunks(DESKTOP_CLIPBOARD_CHUNK_BYTES) { writer.write(chunk, DESKTOP_CLIPBOARD_CHUNK_BYTES).await?; } }
        Ok::<_, DesktopError>(())
    }).await.map_err(|_| DesktopError::Timeout)?
}
