use super::*;
impl RemoteDesktopSession {
    pub async fn occupancy(connection:&PabConnection, timeout:Duration)->Result<Option<RequestId>> {
        let mut channel=stream(connection,RemoteDesktopRequest::Occupancy,timeout).await?;
        match response(&mut channel,timeout).await? {
            RemoteDesktopReply::Occupancy{session_id} if session_id.is_none_or(|id|!id.as_uuid().is_nil())=>Ok(session_id),
            _=>Err(DesktopError::Protocol("invalid desktop occupancy reply")),
        }
    }
    pub async fn takeover(connection:&PabConnection, session_id:RequestId, timeout:Duration)->Result<()> {
        if session_id.as_uuid().is_nil(){return Err(DesktopError::Protocol("invalid takeover session"));}
        let mut channel=stream(connection,RemoteDesktopRequest::Takeover{session_id},timeout).await?;
        match response(&mut channel,timeout).await? {
            RemoteDesktopReply::Closed=>Ok(()),
            _=>Err(DesktopError::Protocol("invalid desktop takeover reply")),
        }
    }
    pub async fn was_taken_over(&self)->bool {
        let work=async {
            let mut channel=stream(&self.connection,RemoteDesktopRequest::SessionStatus{session_id:self.opened.config.session_id},Duration::from_secs(2)).await?;
            response(&mut channel,Duration::from_secs(2)).await
        };
        matches!(tokio::time::timeout(Duration::from_secs(2),work).await,Ok(Err(DesktopError::Rejected{code:DesktopErrorCode::TakenOver,..})))
    }
}
