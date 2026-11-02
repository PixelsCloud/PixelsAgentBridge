use super::*;
static EXCHANGES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
impl DesktopHost {
    pub(super) async fn serve_file_clipboard(
        &self,
        owner: OperatorRef,
        account: Option<UserId>,
        version: u16,
        session: RequestId,
        generation: u64,
        header_bytes: u32,
        payload_bytes: u32,
        mut stream: PabBiStream,
    ) -> Result<()> {
        let timeout = Duration::from_secs(12);
        let checked = self
            .entry(owner, account, session, false)
            .and_then(|entry| {
                if version != FILE_CLIPBOARD_VERSION
                    || !self
                        .shared
                        .backend
                        .as_ref()
                        .is_some_and(|b| b.supports_file_clipboard())
                {
                    return Err(DesktopError::reject(
                        DesktopErrorCode::Unsupported,
                        "file_clipboard_unsupported",
                    ));
                }
                if header_bytes == 0
                    || header_bytes as usize > FILE_CLIPBOARD_HEADER
                    || payload_bytes as usize > FILE_CLIPBOARD_CHUNK
                {
                    return Err(DesktopError::Protocol("file_clipboard_frame_too_large"));
                }
                let video = entry.video(generation)?;
                if !entry.control_attached.load(Ordering::Acquire)
                    || *video.cancelled.borrow()
                    || !video.frame_ready.load(Ordering::Acquire)
                {
                    return Err(DesktopError::Closed);
                }
                Ok((entry, video))
            });
        let (entry, video) = match checked {
            Ok(v) => v,
            Err(error) => return rejected(&mut stream, error, timeout).await,
        };
        let _permit = tokio::select! {_=cancelled(&entry.cancelled)=>return Err(DesktopError::Closed),_=cancelled(&video.cancelled)=>return Err(DesktopError::Closed),value=tokio::time::timeout(timeout,EXCHANGES.acquire())=>value.map_err(|_|DesktopError::Timeout)?.map_err(|_|DesktopError::Closed)?};
        reply(&mut stream, RemoteDesktopReply::Ready, timeout).await?;
        let (mut writer, mut reader) = stream.into_framed();
        let backend = self.shared.backend.as_ref().ok_or(DesktopError::Closed)?;
        let work = async {
            let header =
                crate::file_clipboard_wire::read(&mut reader, header_bytes, FILE_CLIPBOARD_HEADER)
                    .await?;
            let mut operation: FileClipboardOperation = serde_json::from_slice(&header)?;
            operation.validate().map_err(DesktopError::Protocol)?;
            if let FileClipboardOperation::Deliver { bytes, data, .. } = &mut operation {
                if *bytes != payload_bytes {
                    return Err(DesktopError::Protocol("file_clipboard_invalid_reply"));
                }
                *data = crate::file_clipboard_wire::read(
                    &mut reader,
                    payload_bytes,
                    FILE_CLIPBOARD_CHUNK,
                )
                .await?;
            } else if payload_bytes != 0 {
                return Err(DesktopError::Protocol("file_clipboard_unexpected_payload"));
            }
            tokio::select! {_=reader.read(1)=>Err(DesktopError::Closed),result=backend.file_clipboard(video.config.clone(),operation)=>result}
        };
        let result = tokio::select! {biased;_=cancelled(&entry.cancelled)=>Err(DesktopError::Closed),_=cancelled(&video.cancelled)=>Err(DesktopError::Closed),value=tokio::time::timeout(timeout,work)=>value.unwrap_or(Err(DesktopError::Timeout))};
        let result = result.and_then(|s| {
            crate::file_clipboard_wire::verify(&s)?;
            Ok(s)
        });
        tokio::time::timeout(timeout, async {
            match result {
                Ok(snapshot) => {
                    let header = serde_json::to_vec(&snapshot)?;
                    if header.len() > FILE_CLIPBOARD_HEADER {
                        return Err(DesktopError::Protocol("file_clipboard_frame_too_large"));
                    }
                    let reply = DeviceTaskResponse::RemoteDesktop {
                        reply: RemoteDesktopReply::FileClipboard {
                            header_bytes: header.len() as u32,
                            payload_bytes: snapshot.bytes,
                        },
                    };
                    writer
                        .write(&serde_json::to_vec(&reply)?, DESKTOP_CONTROL_BYTES)
                        .await?;
                    crate::file_clipboard_wire::write(&mut writer, &header).await?;
                    crate::file_clipboard_wire::write(&mut writer, &snapshot.data).await?;
                }
                Err(error) => {
                    writer
                        .write(
                            &serde_json::to_vec(&DeviceTaskResponse::RemoteDesktop {
                                reply: RemoteDesktopReply::Error {
                                    code: error.code(),
                                    message: error.to_string(),
                                },
                            })?,
                            DESKTOP_CONTROL_BYTES,
                        )
                        .await?
                }
            }
            Ok::<(), DesktopError>(())
        })
        .await
        .map_err(|_| DesktopError::Timeout)?
    }
}
