use super::*;
use crate::DesktopCursorWriter;

impl DesktopHost {
    pub(super) async fn serve_cursor(
        &self,
        owner: OperatorRef,
        account: Option<UserId>,
        version: u16,
        session_id: RequestId,
        generation: u64,
        mut stream: PabBiStream,
        timeout: Duration,
    ) -> Result<()> {
        let validation = self
            .entry(owner, account, session_id, false)
            .and_then(|entry| {
                if version != DESKTOP_CURSOR_SCHEMA_VERSION
                    || self.supported_cursor_version() != Some(version)
                {
                    return Err(DesktopError::reject(
                        DesktopErrorCode::Unsupported,
                        "independent cursor version is unavailable",
                    ));
                }
                if !entry.control_attached.load(Ordering::SeqCst) {
                    return Err(DesktopError::Protocol("attach control before cursor"));
                }
                let video = entry.video(generation)?;
                if !video.claimed.load(Ordering::SeqCst) {
                    return Err(DesktopError::Protocol("attach video before cursor"));
                }
                if *video.cancelled.borrow() || video.cursor_claimed.swap(true, Ordering::SeqCst) {
                    return Err(DesktopError::reject(
                        DesktopErrorCode::StaleGeneration,
                        "cursor already attached or ended; restart its generation",
                    ));
                }
                Ok((entry, video))
            });
        let (entry, video) = match validation {
            Ok(value) => value,
            Err(error) => return rejected(&mut stream, error, timeout).await,
        };
        let _guard = CursorGuard {
            host: self.clone(),
            entry: entry.clone(),
            video: video.clone(),
        };
        tokio::select! {
            biased;
            _ = cancelled(&entry.cancelled) => Ok(()),
            _ = cancelled(&video.cancelled) => Ok(()),
            result = self.cursor(&video, stream, timeout) => result,
        }
    }

    async fn cursor(
        &self,
        video: &Generation,
        mut stream: PabBiStream,
        timeout: Duration,
    ) -> Result<()> {
        let backend = self.shared.backend.as_ref().ok_or(DesktopError::Closed)?;
        let mut source =
            match tokio::time::timeout(MEDIA_TIMEOUT, backend.cursor(video.config.clone())).await {
                Ok(Ok(source)) => source,
                Ok(Err(error)) => return rejected(&mut stream, error, timeout).await,
                Err(_) => {
                    return rejected(
                        &mut stream,
                        DesktopError::reject(
                            DesktopErrorCode::MediaUnavailable,
                            "cursor source startup timed out",
                        ),
                        timeout,
                    )
                    .await;
                }
            };
        reply(
            &mut stream,
            RemoteDesktopReply::CursorReady {
                version: DESKTOP_CURSOR_SCHEMA_VERSION,
            },
            timeout,
        )
        .await?;
        let (writer, mut reverse) = stream.into_framed();
        let mut writer = DesktopCursorWriter::new(writer, video.config.clone())?;
        loop {
            // Keep the same read alive through snapshot acquisition and send.
            // Dropping/restarting a partially read ACK would lose framing.
            let ack = reverse.read(8);
            tokio::pin!(ack);
            // Between snapshots there is nothing to acknowledge. EOF cancels
            // an idle source promptly; unsolicited/repeated ACKs are invalid.
            let update = tokio::select! {
                biased;
                unexpected = &mut ack => return match unexpected {
                    Ok(_) => Err(DesktopError::Protocol("unsolicited cursor consumption acknowledgment")),
                    Err(_) => Ok(()),
                },
                update = source.next_update() => update?,
            };
            writer.send(&update, timeout).await?;
            // One outstanding snapshot, independent of VIDEO credit. Without
            // this ACK a reliable transport could buffer thousands of old
            // positions, even though the native source retains only its latest.
            let ack = tokio::time::timeout(timeout, &mut ack)
                .await
                .map_err(|_| DesktopError::Timeout)??;
            if ack.as_slice() != update.header.sequence.to_le_bytes() {
                return Err(DesktopError::Protocol(
                    "invalid cursor consumption acknowledgment",
                ));
            }
        }
    }
}

struct CursorGuard {
    host: DesktopHost,
    entry: Arc<Entry>,
    video: Arc<Generation>,
}
impl Drop for CursorGuard {
    fn drop(&mut self) {
        // The source future is dropped before this guard. A cursor error alone
        // does not stop video/input or release the shared single-viewer slot.
        self.video.cursor_stopped.send_replace(true);
        if *self.entry.cancelled.borrow() {
            self.host.release(&self.entry);
        }
    }
}
