use super::*;
// Bound aggregate in-flight image memory independently of keyboard/video.
static TRANSFERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

impl DesktopHost {
    pub(super) async fn serve_clipboard(&self, owner: OperatorRef, account: Option<UserId>, version: u16,
        session_id: RequestId, generation: u64, request: DesktopClipboardRequest, mut stream: PabBiStream, _timeout: Duration) -> Result<()> {
        let timeout = Duration::from_secs(DESKTOP_CLIPBOARD_TIMEOUT_SECS);
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
        let _permit = tokio::select! {
            _=cancelled(&entry.cancelled)=>return Err(DesktopError::Closed),
            _=cancelled(&video.cancelled)=>return Err(DesktopError::Closed),
            permit=tokio::time::timeout(timeout,TRANSFERS.acquire())=>permit.map_err(|_|DesktopError::Timeout)?.map_err(|_|DesktopError::Closed)?,
        };
        // A failed clipboard exchange owns only this stream, never the video/input lease.
        let operation = match request {
            DesktopClipboardRequest::Read { after } => DesktopClipboardOperation::Read { after },
            DesktopClipboardRequest::WriteImage { mut image, expected_revision } => {
                if let Err(error) = image.validate_header() { return rejected(&mut stream, DesktopError::Protocol(error), timeout).await; }
                reply(&mut stream, RemoteDesktopReply::Ready, timeout).await?;
                let (writer, mut reader) = stream.into_framed();
                let work = async {
                    while image.data.len() < image.bytes as usize {
                        let chunk = reader.read(DESKTOP_CLIPBOARD_CHUNK_BYTES.min(image.bytes as usize - image.data.len())).await?;
                        if chunk.is_empty() { return Err(DesktopError::Protocol("empty clipboard chunk")); }
                        image.data.extend_from_slice(&chunk);
                    }
                    image.validate().map_err(DesktopError::Protocol)?;
                    tokio::select! {
                        _=reader.read(1)=>Err(DesktopError::Closed),
                        result=backend.clipboard(video.config.clone(),DesktopClipboardOperation::WriteImage{image,expected_revision})=>result,
                    }
                };
                let result = tokio::select! {
                    biased;
                    _ = cancelled(&entry.cancelled) => Err(DesktopError::Closed),
                    _ = cancelled(&video.cancelled) => Err(DesktopError::Closed),
                    result = tokio::time::timeout(timeout, work) => result.unwrap_or(Err(DesktopError::Timeout)),
                };
                return send_snapshot(writer, result, timeout).await;
            }
            DesktopClipboardRequest::Write { bytes } => {
                if bytes as usize > DESKTOP_CLIPBOARD_MAX_BYTES { return rejected(&mut stream, DesktopError::Protocol("clipboard text exceeds 1 MiB"), timeout).await; }
                reply(&mut stream, RemoteDesktopReply::Ready, timeout).await?;
                let (writer, mut reader) = stream.into_framed();
                let work = async {
                    let mut text = Vec::with_capacity(bytes as usize);
                    while text.len() < bytes as usize {
                        let chunk=reader.read(DESKTOP_CLIPBOARD_CHUNK_BYTES.min(bytes as usize-text.len())).await?;
                        if chunk.is_empty(){return Err(DesktopError::Protocol("empty clipboard chunk"));}text.extend_from_slice(&chunk);
                    }
                    let text = String::from_utf8(text).map_err(|_| DesktopError::Protocol("clipboard text is not UTF-8"))?;
                    validate_clipboard_text(&text).map_err(DesktopError::Protocol)?;
                    tokio::select! {
                        _=reader.read(1)=>Err(DesktopError::Closed),
                        result=backend.clipboard(video.config.clone(),DesktopClipboardOperation::Write{text})=>result,
                    }
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
        let (writer,mut reader)=stream.into_framed();
        let result = tokio::select! {
            biased;
            _=reader.read(1)=>Err(DesktopError::Closed),
            _ = cancelled(&entry.cancelled) => Err(DesktopError::Closed),
            _ = cancelled(&video.cancelled) => Err(DesktopError::Closed),
            result = tokio::time::timeout(timeout, backend.clipboard(video.config.clone(), operation)) => result.unwrap_or(Err(DesktopError::Timeout)),
        };
        send_snapshot(writer, result, timeout).await
    }
}

async fn send_snapshot(mut writer: pab_transport::PabFrameWriter, result: Result<DesktopClipboardSnapshot>, timeout: Duration) -> Result<()> {
    let result = result.and_then(|snapshot| { snapshot.validate().map_err(DesktopError::Protocol)?; Ok(snapshot) });
    let (header, payload) = match result {
        Ok(mut snapshot) if snapshot.image.is_some() => {
            let mut image = snapshot.image.take().unwrap(); let payload = std::mem::take(&mut image.data);
            (RemoteDesktopReply::Clipboard { version: DESKTOP_CLIPBOARD_VERSION, revision: snapshot.revision,
                changed: snapshot.changed, bytes: None, image: Some(image) }, Some(payload))
        }
        Ok(snapshot) => (RemoteDesktopReply::Clipboard { version: DESKTOP_CLIPBOARD_VERSION, revision: snapshot.revision,
            changed: snapshot.changed, bytes: snapshot.text.as_ref().map(|text| text.len() as u32), image: None }, snapshot.text.map(String::into_bytes)),
        Err(error) => (RemoteDesktopReply::Error { code: error.code(), message: error.to_string() }, None),
    };
    tokio::time::timeout(timeout, async {
        writer.write(&serde_json::to_vec(&DeviceTaskResponse::RemoteDesktop { reply: header })?, DESKTOP_CONTROL_BYTES).await?;
        if let Some(payload) = payload { for chunk in payload.chunks(DESKTOP_CLIPBOARD_CHUNK_BYTES) { writer.write(chunk, DESKTOP_CLIPBOARD_CHUNK_BYTES).await?; } }
        Ok::<_, DesktopError>(())
    }).await.map_err(|_| DesktopError::Timeout)?
}
