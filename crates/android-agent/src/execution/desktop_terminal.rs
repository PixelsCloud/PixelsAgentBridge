//! Human terminals use the streaming protocol, with bounded PTY backpressure.
//! Sessions belong to one authenticated connection and always run as the app UID.
use super::*;
use pab_interactive_terminal::{InteractivePty, PtyOutput};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

pub(super) struct Entry {
    owner: String,
    scope: Arc<ConnectionScope>,
    opened: DesktopTerminalOpened,
    pty: Arc<InteractivePty>,
    output: Mutex<Option<mpsc::Receiver<PtyOutput>>>,
    control_attached: AtomicBool,
    closed: watch::Sender<bool>,
    close_lock: Mutex<()>,
}
impl Drop for Entry {
    fn drop(&mut self) { let _ = self.pty.kill(); }
}
type WireResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

impl ExecutionService {
    async fn desktop_open(&self, owner: &str, scope: &Arc<ConnectionScope>, id: RequestId,
        version: u16, execution: ExecutionSelection, cols: u16, rows: u16) -> Result<Arc<Entry>> {
        if version != DESKTOP_TERMINAL_SCHEMA_VERSION || validate_desktop_terminal_size(cols,rows).is_err() {
            return Err(invalid("invalid_desktop_terminal_request"));
        }
        if !execution.is_service() { return Err(denied("android_terminal_requires_app_uid")); }
        let _accept = self.acceptance.lock().await;
        let digest = self.digest(&serde_json::to_vec(&("desktop", version, execution, cols, rows)).map_err(|_|invalid("invalid_desktop_terminal_request"))?);
        if let Some(row) = sqlx::query("SELECT owner,digest FROM terminals WHERE id=?").bind(id.to_string()).fetch_optional(&self.pool).await? {
            if row.get::<String,_>("owner") != owner || row.get::<String,_>("digest") != digest { return Err(invalid("request_conflict")); }
            return self.desktop_entry(owner,scope,id).await;
        }
        let (root,uid) = self.authorize().await?;
        if scope.closed.load(Ordering::Acquire) || self.stopping.load(Ordering::Acquire) { return Err(denied("terminal_connection_closed")); }
        if self.desktop_terminals.lock().await.len() >= 2 { return Err(denied("desktop_terminal_capacity")); }
        let count:i64 = sqlx::query_scalar("SELECT COUNT(*) FROM terminals").fetch_one(&self.pool).await?;
        if count>=10000 { return Err(internal("terminal_journal_capacity")); }
        sqlx::query("INSERT INTO terminals(owner,id,digest,state) VALUES(?,?,?,'accepted')").bind(owner).bind(id.to_string()).bind(digest).execute(&self.pool).await?;
        let arguments = vec!["-i".to_owned()];
        let (pty,output) = InteractivePty::start("/system/bin/sh",&arguments,&root,cols,rows)
            .map_err(|_|internal("android_pty_start_failed"))?;
        let entry=Arc::new(Entry {owner:owner.into(),scope:scope.clone(),opened:DesktopTerminalOpened {
            session_id:id,shell:"/system/bin/sh".into(),arguments,cwd:root.to_string_lossy().into_owned(),identity:identity(uid,&root),cols,rows,
        },pty,output:Mutex::new(Some(output)),control_attached:AtomicBool::new(false),closed:watch::channel(false).0,close_lock:Mutex::new(())});
        self.desktop_terminals.lock().await.insert(id,entry.clone());
        if scope.closed.load(Ordering::Acquire) || self.stopping.load(Ordering::Acquire) {
            self.close_desktop_entry(id,entry).await;return Err(denied("terminal_connection_closed"));
        }
        if sqlx::query("UPDATE terminals SET state='running' WHERE id=?").bind(id.to_string()).execute(&self.pool).await.is_err() {
            self.close_desktop_entry(id,entry).await;return Err(internal("terminal_open_outcome_unconfirmed"));
        }
        Ok(entry)
    }
    async fn desktop_entry(&self,owner:&str,scope:&Arc<ConnectionScope>,id:RequestId)->Result<Arc<Entry>> {
        let entry=self.desktop_terminals.lock().await.get(&id).cloned().ok_or_else(missing)?;
        if entry.owner!=owner || entry.scope.id!=scope.id || scope.closed.load(Ordering::Acquire) || *entry.closed.borrow() {return Err(missing());}
        Ok(entry)
    }
    async fn close_desktop_entry(&self,id:RequestId,entry:Arc<Entry>) {
        let _close=entry.close_lock.lock().await;
        entry.closed.send_replace(true);
        let pty=entry.pty.clone();let _=tokio::task::spawn_blocking(move ||pty.kill()).await;
        entry.output.lock().await.take();
        self.desktop_terminals.lock().await.remove(&id);
        let _=sqlx::query("UPDATE terminals SET state='interrupted' WHERE id=?").bind(id.to_string()).execute(&self.pool).await;
    }
    pub(super) async fn close_desktop_terminals(&self,scope:Option<&str>) {
        let entries:Vec<_>=self.desktop_terminals.lock().await.iter().filter(|(_,e)|scope.is_none_or(|s|s==e.scope.id)).map(|(id,e)|(*id,e.clone())).collect();
        for (id,entry) in entries { self.close_desktop_entry(id,entry).await; }
    }
    pub(crate) async fn serve_desktop_terminal(self:&Arc<Self>,owner:&str,scope:&Arc<ConnectionScope>,request:DesktopTerminalRequest,mut stream:PabBiStream)->std::result::Result<(),PabConnectionError> {
        let timeout=Duration::from_secs(8);
        if let DesktopTerminalRequest::Close {session_id}=request {
            if self.desktop_entry(owner,scope,session_id).await.is_err() {
                let closed=sqlx::query_scalar::<_,String>("SELECT state FROM terminals WHERE owner=? AND id=?").bind(owner).bind(session_id.to_string()).fetch_optional(&self.pool).await;
                if matches!(closed,Ok(Some(state)) if state!="running" && state!="accepted") {
                    return stream.send_frame_json(&DeviceTaskResponse::DesktopTerminal {reply:DesktopTerminalReply::Closed},timeout).await;
                }
            }
        }
        let (id,entry)=match request.clone() {
            DesktopTerminalRequest::Open {session_id,version,execution,cols,rows} => (session_id,self.desktop_open(owner,scope,session_id,version,execution,cols,rows).await),
            DesktopTerminalRequest::Control {session_id}|DesktopTerminalRequest::Output {session_id}|DesktopTerminalRequest::Close {session_id}=>(session_id,self.desktop_entry(owner,scope,session_id).await),
        };
        let entry=match entry {
            Ok(entry)=>entry,
            Err(error)=>{stream.send_frame_json(&DeviceTaskResponse::DesktopTerminal {reply:DesktopTerminalReply::Error {message:error.1.into()}},timeout).await?;return Ok(());}
        };
        let reply=match request {DesktopTerminalRequest::Open {..}=>DesktopTerminalReply::Opened {opened:entry.opened.clone()},DesktopTerminalRequest::Close {..}=>DesktopTerminalReply::Closed,_=>DesktopTerminalReply::Ready};
        if matches!(request,DesktopTerminalRequest::Close {..}) {self.close_desktop_entry(id,entry.clone()).await;}
        if let Err(error)=stream.send_frame_json(&DeviceTaskResponse::DesktopTerminal {reply},timeout).await {self.close_desktop_entry(id,entry).await;return Err(error);}
        match request {
            DesktopTerminalRequest::Open {..}=>{
                let service=self.clone();let weak=Arc::downgrade(&entry);
                tokio::spawn(async move {tokio::time::sleep(Duration::from_secs(30)).await;if let Some(entry)=weak.upgrade() {
                    if entry.output.lock().await.is_some() || !entry.control_attached.load(Ordering::Acquire) {service.close_desktop_entry(id,entry).await;}
                }});Ok(())
            }
            DesktopTerminalRequest::Close {..}=>Ok(()),
            DesktopTerminalRequest::Control {..}=>{
                if entry.control_attached.swap(true,Ordering::AcqRel) {return Err(PabConnectionError::Stream("terminal control already attached".into()));}
                let result=self.desktop_controls(&entry,stream).await;
                self.close_desktop_entry(id,entry).await;
                result.map_err(|e|PabConnectionError::Stream(e.to_string()))
            }
            DesktopTerminalRequest::Output {..}=>{
                let output=entry.output.lock().await.take();
                let Some(output)=output else{return Err(PabConnectionError::Stream("terminal output already attached".into()));};
                let result=desktop_output(&entry,output,stream).await;
                self.close_desktop_entry(id,entry).await;
                result.map_err(|e|PabConnectionError::Stream(e.to_string()))
            }
        }
    }
    async fn desktop_controls(&self,entry:&Entry,stream:PabBiStream)->WireResult<()> {
        let (mut writer,mut reader)=stream.into_framed();let mut cancelled=entry.closed.subscribe();let mut sequence=1;
        loop {
            let bytes=tokio::select! {value=reader.read(24*1024)=>value?,_=cancelled.wait_for(|v|*v)=>return Ok(())};
            let request:DesktopTerminalControl=serde_json::from_slice(&bytes)?;request.validate()?;
            self.authorize().await.map_err(|e|e.1)?;
            let pty=entry.pty.clone();
            match request {
                DesktopTerminalControl::Input {sequence:current,bytes}=>{
                    if current!=sequence {return Err("terminal input sequence mismatch".into());}
                    sequence=sequence.checked_add(1).ok_or("terminal sequence exhausted")?;
                    tokio::task::spawn_blocking(move ||pty.input(&bytes)).await??;
                }
                DesktopTerminalControl::Resize {cols,rows}=>{tokio::task::spawn_blocking(move ||pty.resize(cols,rows)).await??;}
            }
            writer.write(&serde_json::to_vec(&DesktopTerminalReply::Applied)?,4096).await?;
        }
    }
}
struct AbortReader(tokio::task::JoinHandle<()>);
impl Drop for AbortReader {fn drop(&mut self){self.0.abort();}}
fn consume(credit:&mut DesktopTerminalCredit,value:&[u8])->WireResult<()> {
    credit.consume(u64::from_be_bytes(value.try_into().map_err(|_|"invalid terminal ACK")?))?;Ok(())
}
async fn desktop_output(entry:&Entry,mut output:mpsc::Receiver<PtyOutput>,stream:PabBiStream)->WireResult<()> {
    let (mut writer,mut reader)=stream.into_framed();let (tx,mut acks)=mpsc::channel(16);
    let _reader=AbortReader(tokio::spawn(async move {loop {let value=reader.read(8).await;let failed=value.is_err();if tx.send(value).await.is_err()||failed {return;}}}));
    let mut cancelled=entry.closed.subscribe();let mut credit=DesktopTerminalCredit::default();let mut hash=Sha256::new();
    loop {
        while credit.available()<DESKTOP_TERMINAL_CHUNK as u64 {
            let value=tokio::select!{v=acks.recv()=>v.ok_or("terminal ACK channel ended")??,_=cancelled.wait_for(|v|*v)=>return Ok(())};consume(&mut credit,&value)?;
        }
        let value=tokio::select!{v=output.recv()=>v.ok_or("PTY output ended without EOF")?,_=cancelled.wait_for(|v|*v)=>return Ok(())};
        match value {
            PtyOutput::Bytes(data)=>{
                let offset=credit.reserve(data.len())?;hash.update(&data);let mut frame=vec![0];frame.extend_from_slice(&offset.to_be_bytes());frame.extend_from_slice(&data);
                tokio::select!{v=writer.write(&frame,DESKTOP_TERMINAL_CHUNK+9)=>v?,_=cancelled.wait_for(|v|*v)=>return Ok(())};
            }
            end=>{
                while credit.available()!=DESKTOP_TERMINAL_CREDIT {
                    let value=tokio::select!{v=acks.recv()=>v.ok_or("terminal ACK channel ended")??,_=cancelled.wait_for(|v|*v)=>return Ok(())};consume(&mut credit,&value)?;
                }
                let reason=match end {PtyOutput::Error(message)=>message,_=>"shell exited".into()};
                let data=json!({"reason":reason,"exit_code":entry.pty.exit_code().ok().flatten(),"output_bytes":credit.sent(),"sha256":format!("{:x}",hash.finalize())});
                let mut frame=vec![1];frame.extend_from_slice(&credit.sent().to_be_bytes());frame.extend_from_slice(&serde_json::to_vec(&data)?);
                writer.write(&frame,4105).await?;writer.finish()?;return Ok(());
            }
        }
        while let Ok(value)=acks.try_recv(){consume(&mut credit,&value?)?;}
    }
}
