use super::*;

pub(super) struct ControlGuard { pub host: DesktopHost, pub entry: Arc<Entry> }
impl Drop for ControlGuard {
    fn drop(&mut self) {
        self.entry.control_stopped.send_replace(true);
        self.host.release(&self.entry);
    }
}
impl DesktopHost {
    pub(super) async fn serve_takeover(&self, account:Option<UserId>, request:RemoteDesktopRequest, mut stream:PabBiStream, timeout:Duration)->Result<()> {
        // Device authentication is already enforced by the parent connection;
        // additionally require its current signed-in account for takeover.
        if account.is_none() || self.connection.closed.load(Ordering::SeqCst) {
            return rejected(&mut stream,DesktopError::reject(DesktopErrorCode::WrongOwner,"sign in before taking over a desktop"),timeout).await;
        }
        match request {
            RemoteDesktopRequest::Occupancy => {
                let session_id=self.shared.slot.lock().unwrap().as_ref().map(|entry|entry.id);
                reply(&mut stream,RemoteDesktopReply::Occupancy{session_id},timeout).await
            }
            RemoteDesktopRequest::SessionStatus{session_id} => {
                let taken_over=*self.connection.taken_over.lock().unwrap()==Some(session_id);
                if taken_over {rejected(&mut stream,DesktopError::reject(DesktopErrorCode::TakenOver,"This desktop session was taken over by another controller."),timeout).await}
                else{reply(&mut stream,RemoteDesktopReply::Ready,timeout).await}
            }
            RemoteDesktopRequest::Takeover{session_id} => {
                let selected={
                    let slot=self.shared.slot.lock().unwrap();
                    match slot.as_ref() {
                        Some(entry) if entry.id==session_id && !session_id.as_uuid().is_nil()=>{
                            *entry.taken_over.lock().unwrap()=Some(entry.id);
                            entry.stop();
                            Ok(Some(entry.clone()))
                        }
                        None=>Ok(None),
                        _=>Err(DesktopError::reject(DesktopErrorCode::Busy,"The desktop owner changed; confirm takeover again.")),
                    }
                };
                let selected=match selected{Ok(value)=>value,Err(error)=>return rejected(&mut stream,error,timeout).await};
                if let Some(entry)=selected {
                    self.release(&entry);
                    let video=entry.state.lock().unwrap().as_ref().map(|ready|ready.video.clone());
                    let stopped=async {
                        if let Some(video)=video {video.wait_stopped().await;}
                        if entry.control_attached.load(Ordering::SeqCst){cancelled(&entry.control_stopped).await;}
                    };
                    if tokio::time::timeout(MEDIA_TIMEOUT,stopped).await.is_err(){return rejected(&mut stream,DesktopError::Timeout,timeout).await;}
                    self.release(&entry);
                }
                reply(&mut stream,RemoteDesktopReply::Closed,timeout).await
            }
            _=>unreachable!(),
        }
    }
}
