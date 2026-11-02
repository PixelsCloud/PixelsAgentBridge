use super::*;

#[derive(Clone)]
pub struct RemoteDesktopClipboardControl {
    connection: PabConnection,
    config: DesktopVideoConfig,
    ended: watch::Receiver<bool>,
    supported: bool,
    file_supported:bool,
}
impl RemoteDesktopSession {
    pub fn clipboard_control(&self) -> RemoteDesktopClipboardControl {
        RemoteDesktopClipboardControl { connection: self.connection.clone(), config: self.opened.config.clone(), ended: self.generation_ended.subscribe(), supported: self.clipboard_version == Some(DESKTOP_CLIPBOARD_VERSION),file_supported:self.file_clipboard_version==Some(FILE_CLIPBOARD_VERSION) }
    }
}
impl RemoteDesktopClipboardControl {
    pub fn files_supported(&self)->bool{self.file_supported}
    pub async fn files(&mut self,operation:FileClipboardOperation)->Result<FileClipboardSnapshot>{
        if !self.file_supported{return Err(DesktopError::reject(DesktopErrorCode::Unsupported,"file_clipboard_unsupported"));}
        if *self.ended.borrow()||self.ended.has_changed().is_err(){return Err(DesktopError::Closed);}
        operation.validate().map_err(DesktopError::Protocol)?;
        let header=serde_json::to_vec(&operation)?;if header.len()>FILE_CLIPBOARD_HEADER{return Err(DesktopError::Protocol("file_clipboard_too_many_files"));}
        let timeout=Duration::from_secs(12);
        let work=async{
            let mut channel=stream(&self.connection,RemoteDesktopRequest::FileClipboard{version:FILE_CLIPBOARD_VERSION,session_id:self.config.session_id,generation:self.config.generation,header_bytes:header.len() as u32,payload_bytes:operation.payload().len() as u32},timeout).await?;
            if !matches!(response(&mut channel,timeout).await?,RemoteDesktopReply::Ready){return Err(DesktopError::Protocol("file_clipboard_not_ready"));}
            let(mut writer,mut reader)=channel.into_framed();crate::file_clipboard_wire::write(&mut writer,&header).await?;crate::file_clipboard_wire::write(&mut writer,operation.payload()).await?;
            let response=reader.read(DESKTOP_CONTROL_BYTES).await?;
            let (header_bytes,payload_bytes)=match serde_json::from_slice::<DeviceTaskResponse>(&response)?{
                DeviceTaskResponse::RemoteDesktop{reply:RemoteDesktopReply::FileClipboard{header_bytes,payload_bytes}}=>(header_bytes,payload_bytes),
                DeviceTaskResponse::RemoteDesktop{reply:RemoteDesktopReply::Error{code,message}}=>return Err(DesktopError::reject(code,message)),_=>return Err(DesktopError::Protocol("file_clipboard_invalid_reply"))};
            let header=crate::file_clipboard_wire::read(&mut reader,header_bytes,FILE_CLIPBOARD_HEADER).await?;
            let mut snapshot:FileClipboardSnapshot=serde_json::from_slice(&header)?;snapshot.validate().map_err(DesktopError::Protocol)?;
            if snapshot.bytes!=payload_bytes{return Err(DesktopError::Protocol("file_clipboard_invalid_reply"));}
            snapshot.data=crate::file_clipboard_wire::read(&mut reader,payload_bytes,FILE_CLIPBOARD_CHUNK).await?;crate::file_clipboard_wire::verify(&snapshot)?;Ok(snapshot)
        };
        tokio::select!{biased;_=self.ended.changed()=>Err(DesktopError::Closed),result=tokio::time::timeout(timeout,work)=>result.map_err(|_|DesktopError::Timeout)?}
    }
    pub fn is_supported(&self) -> bool { self.supported }
    pub async fn exchange(&mut self, operation: DesktopClipboardOperation) -> Result<DesktopClipboardSnapshot> {
        if !self.supported { return Err(DesktopError::reject(DesktopErrorCode::Unsupported, "remote desktop clipboard requires an updated remote client")); }
        operation.validate().map_err(DesktopError::Protocol)?;
        if *self.ended.borrow() || self.ended.has_changed().is_err() { return Err(DesktopError::Closed); }
        let timeout = Duration::from_secs(DESKTOP_CLIPBOARD_TIMEOUT_SECS);
        let request = match &operation {
            DesktopClipboardOperation::Read { after } => DesktopClipboardRequest::Read { after: *after },
            DesktopClipboardOperation::Write { text } => DesktopClipboardRequest::Write { bytes: text.len() as u32 },
            DesktopClipboardOperation::WriteImage { image, expected_revision } => {
                let header = DesktopClipboardImage { width:image.width,height:image.height,bytes:image.bytes,data:Vec::new() };
                DesktopClipboardRequest::WriteImage { image: header, expected_revision: *expected_revision }
            },
        };
        let work = async {
            let mut stream = stream(&self.connection, RemoteDesktopRequest::Clipboard { version: DESKTOP_CLIPBOARD_VERSION,
                session_id: self.config.session_id, generation: self.config.generation, request }, timeout).await?;
            let payload = match &operation { DesktopClipboardOperation::Write { text } => Some(text.as_bytes()),
                DesktopClipboardOperation::WriteImage { image, .. } => Some(image.data.as_slice()), _ => None };
            if let Some(payload) = payload {
                if !matches!(response(&mut stream, timeout).await?, RemoteDesktopReply::Ready) { return Err(DesktopError::Protocol("unexpected clipboard write reply")); }
                let (mut writer, reader) = stream.into_framed();
                for chunk in payload.chunks(DESKTOP_CLIPBOARD_CHUNK_BYTES) { writer.write(chunk, DESKTOP_CLIPBOARD_CHUNK_BYTES).await?; }
                return read_snapshot(reader, timeout).await;
            }
            let (_writer, reader) = stream.into_framed();
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
    let (revision, changed, size, mut image) = match reply {
        DeviceTaskResponse::RemoteDesktop { reply: RemoteDesktopReply::Clipboard { version: DESKTOP_CLIPBOARD_VERSION, revision, changed, bytes, image } } => (revision, changed, bytes, image),
        DeviceTaskResponse::RemoteDesktop { reply: RemoteDesktopReply::Error { code, message } } => return Err(DesktopError::reject(code, message)),
        _ => return Err(DesktopError::Protocol("unexpected clipboard reply")),
    };
    if revision == 0 || size.is_some_and(|size| size as usize > DESKTOP_CLIPBOARD_MAX_BYTES) || (!changed && (size.is_some() || image.is_some())) || (size.is_some() && image.is_some()) {
        return Err(DesktopError::Protocol("invalid clipboard reply"));
    }
    let text = if let Some(size) = size {
        let mut text = Vec::with_capacity(size as usize);
        while text.len() < size as usize {
            let chunk=reader.read(DESKTOP_CLIPBOARD_CHUNK_BYTES.min(size as usize-text.len())).await?;
            if chunk.is_empty(){return Err(DesktopError::Protocol("empty clipboard chunk"));}
            text.extend_from_slice(&chunk);
        }
        let text = String::from_utf8(text).map_err(|_| DesktopError::Protocol("clipboard text is not UTF-8"))?;
        validate_clipboard_text(&text).map_err(DesktopError::Protocol)?;
        Some(text)
    } else { None };
    if let Some(image) = &mut image {
        image.validate_header().map_err(DesktopError::Protocol)?;
        while image.data.len() < image.bytes as usize {
            let chunk = reader.read(DESKTOP_CLIPBOARD_CHUNK_BYTES.min(image.bytes as usize - image.data.len())).await?;
            if chunk.is_empty() { return Err(DesktopError::Protocol("empty clipboard chunk")); }
            image.data.extend_from_slice(&chunk);
        }
        image.validate().map_err(DesktopError::Protocol)?;
    }
    Ok(DesktopClipboardSnapshot { revision, changed, text, image })
}
