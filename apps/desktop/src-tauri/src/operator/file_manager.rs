//! Independent file windows are bound to a verified account/runtime. The
//! WebView cannot supply its own owner or another window's request IDs.
use super::{OperatorState,parse_code};
use pab_bridge::BridgeRuntime;
use pab_file_manager::LocalFileManager;
use pab_protocol::{DeviceRef,ExecutionSelection,FileManagerOperation as Op,FileManagerRequest,FileManagerReply,FileManagerResult,NativeFilePath,PathStyle,RequestId};
use serde::Serialize;
use std::{collections::{HashMap,VecDeque},sync::{Arc,atomic::{AtomicBool,Ordering}},time::{Duration,Instant}};
use tauri::{AppHandle,Emitter,Manager,State,WebviewUrl,WebviewWindow,WebviewWindowBuilder};
use tokio::sync::{Mutex,OnceCell,Semaphore,watch};

#[derive(Default)]
pub struct FileManagerState {
    windows: Mutex<HashMap<String,Arc<Owner>>>,
    local: OnceCell<Arc<LocalFileManager>>,
    batch_store:OnceCell<pab_file_manager::BatchStore>,
    jobs:Mutex<HashMap<RequestId,Arc<pab_bridge::ManagedBatchJob>>>,
    exit_ready:AtomicBool,exit_prompt:AtomicBool,exit_stopping:AtomicBool,english:AtomicBool,
}
struct Owner {
    scope:String,
    account:String,
    account_scope:String,
    runtime:Arc<BridgeRuntime>,
    local:Arc<LocalFileManager>,
    devices:Mutex<HashMap<String,Tab>>,
    requests:Mutex<HashMap<RequestId,Arc<Query>>>,
    snapshots:Mutex<Vec<DirectorySnapshot>>,
    slots:Arc<Semaphore>,
    closed:AtomicBool,
    ended:watch::Sender<bool>,
    drops:Mutex<VecDeque<NativeDrop>>,
}
struct NativeDrop {id:RequestId,paths:Vec<NativeFilePath>,created:Instant,batch:Option<RequestId>}
#[derive(Clone,Serialize)]
struct DropNotice {drop_id:RequestId,count:usize,names:Vec<String>,x:f64,y:f64}
struct Query { spec:String, endpoint:String, device:Option<DeviceRef>, tab_generation:Option<RequestId>, request:FileManagerRequest, force_relay:bool, cancel:watch::Sender<bool>, frame:Mutex<Frame> }
struct DirectorySnapshot {endpoint:String,generation:RequestId,device:DeviceRef,execution:ExecutionSelection,relay:bool,id:RequestId}
#[derive(Clone,Serialize)]
pub struct Tab {code:String,name:String,os_family:Option<String>,device:DeviceRef,generation:RequestId}
#[derive(Clone,Serialize)]
pub struct Frame {request_id:RequestId,state:String,reply:Option<FileManagerReply>,message:Option<String>}
fn account()->Result<String,String> {
    let snapshot = pab_agent_core::account::AccountStore::from_env().and_then(|store|store.read()).map_err(|e|e.to_string())?;
    snapshot.user.map(|user|format!("{}:{}",user.id,snapshot.revision)).ok_or_else(||"Sign in to open file transfer".into())
}
impl FileManagerState {
    pub(crate) fn set_english(&self,english:bool){self.english.store(english,Ordering::Release);}
    /// Explicit process exit/restart first offers durable pause. Closing the
    /// file window itself continues to leave accepted work in the background.
    pub(crate) fn intercept_exit(app:&AppHandle,code:Option<i32>)->bool {
        let state=app.state::<Self>();
        if state.exit_ready.load(Ordering::Acquire){return false;}
        if state.jobs.try_lock().is_ok_and(|jobs|jobs.values().all(|job|!job.running())){return false;}
        if state.exit_prompt.swap(true,Ordering::AcqRel){return true;}
        let app=app.clone();
        tauri::async_runtime::spawn(async move {
            let state=app.state::<Self>();let english=state.english.load(Ordering::Acquire);
            let (title,body,confirm,cancel)=if english {("Exit Pixels?","File transfers are running. Pause and save their progress before exiting?","Pause and exit","Keep running")}else{("退出 Pixels？","仍有文件正在传输。是否暂停并保存进度后退出？","暂停并退出","继续运行")};
            if !exit_question(&app,title,body,confirm,cancel).await {state.cancel_exit(&app);return;}
            state.exit_stopping.store(true,Ordering::Release);
            let jobs:Vec<_>=state.jobs.lock().await.values().filter(|job|job.running()).cloned().collect();
            let mut pauses=tokio::task::JoinSet::new();
            for job in jobs.iter().cloned(){pauses.spawn(async move {job.request_intent(pab_protocol::FileTransferIntent::Pause).await});}
            let _=tokio::time::timeout(Duration::from_secs(5),async {while pauses.join_next().await.is_some(){}}).await;
            pauses.abort_all();drop(pauses);
            let stopped=tokio::time::timeout(Duration::from_secs(5),async {while jobs.iter().any(|job|job.running()){tokio::time::sleep(Duration::from_millis(100)).await;}}).await.is_ok();
            if !stopped {
                let (title,body,confirm,cancel)=if english {("Some transfers are unconfirmed","The peer has not confirmed that all transfers stopped. Exiting will keep these tasks for reconciliation on the next start.","Exit and reconcile later","Keep running")}else{("部分任务尚未确认停止","远端尚未确认所有传输已停止。退出后将保留这些任务，下次启动时先核实原请求。","退出，下次核实","继续运行")};
                if !exit_question(&app,title,body,confirm,cancel).await {state.cancel_exit(&app);return;}
                for job in &jobs {job.request_shutdown();}
                let _=tokio::time::timeout(Duration::from_secs(5),async {while jobs.iter().any(|job|job.running()){tokio::time::sleep(Duration::from_millis(100)).await;}}).await;
            }
            state.exit_ready.store(true,Ordering::Release);app.exit(code.unwrap_or(0));
        });
        true
    }
    fn cancel_exit(&self,app:&AppHandle){self.exit_stopping.store(false,Ordering::Release);self.exit_prompt.store(false,Ordering::Release);crate::tray::cancel_exit(app);}
    async fn prune_finished_jobs(&self) {
        if self.jobs.lock().await.len()<96{return;}
        let candidates:Vec<_>=self.jobs.lock().await.iter().filter(|(_,job)|!job.running()).map(|(id,job)|(*id,job.clone())).collect();
        for (id,job) in candidates {
            if job.snapshot().await.is_ok_and(|snapshot|matches!(snapshot.phase.as_str(),"completed"|"completed_with_skips"|"partial_failed"|"failed"|"cancelled")) {
                let mut jobs=self.jobs.lock().await;
                if jobs.get(&id).is_some_and(|current|Arc::ptr_eq(current,&job) && !current.running()){jobs.remove(&id);}
            }
        }
    }
    async fn batches(&self)->Result<pab_file_manager::BatchStore,String> {
        self.batch_store.get_or_try_init(||async {
            let paths=pab_agent_core::DataPaths::for_operator().map_err(|e|e.to_string())?;
            let database=paths.bridge_database().with_file_name("file-manager-batches.sqlite3");
            pab_agent_core::ensure_data_parent(&database).map_err(|e|e.to_string())?;
            pab_file_manager::BatchStore::open(&database).await.map_err(|e|e.to_string())
        }).await.cloned()
    }
    async fn owner(&self,window:&WebviewWindow)->Result<Arc<Owner>,String> {
        let owner = self.windows.lock().await.get(window.label()).cloned().ok_or("File window is closed")?;
        if owner.closed.load(Ordering::Acquire) || owner.account != account()? {return Err("Account changed; reopen file transfer".into());}
        Ok(owner)
    }
    async fn close(&self,label:&str) {
        let Some(owner) = self.windows.lock().await.remove(label) else {return};
        owner.closed.store(true,Ordering::Release);
        owner.ended.send_replace(true);
        for request in owner.requests.lock().await.values() {request.cancel.send_replace(true);}
        owner.cleanup().await;
    }
}
async fn exit_question(app:&AppHandle,title:&str,body:&str,confirm:&str,cancel:&str)->bool {
    let parent=app.get_webview_window("main").filter(|w|w.is_visible().unwrap_or(false))
        .or_else(||app.webview_windows().into_values().find(|w|!w.label().starts_with("confirmation-") && w.is_visible().unwrap_or(false)));
    crate::confirmation::ask(app,parent.as_ref(),title,body,confirm,cancel).await
}
impl Owner {
    async fn cleanup(&self) {
        let _ = self.local.release(&self.scope).await;
        let snapshots = std::mem::take(&mut *self.snapshots.lock().await);
        self.close_snapshots(snapshots).await;
    }
    async fn close_snapshots(&self,snapshots:Vec<DirectorySnapshot>) {
        for snapshot in snapshots {
            let request = FileManagerRequest {expected_execution:None,schema_version:1,request_id:RequestId::new(),execution:snapshot.execution,operation:Op::CloseDirectory {snapshot_id:snapshot.id}};
            let _ = tokio::time::timeout(Duration::from_secs(5),self.runtime.file_manager_profile(snapshot.device,&request,snapshot.relay)).await;
        }
    }
    async fn take_endpoint_snapshots(&self,endpoint:&str,generation:RequestId)->Vec<DirectorySnapshot> {
        let mut snapshots=self.snapshots.lock().await;let (removed,kept)=std::mem::take(&mut *snapshots).into_iter().partition(|snapshot|snapshot.endpoint==endpoint && snapshot.generation==generation);*snapshots=kept;removed
    }
    async fn cleanup_endpoint(&self,endpoint:&str,generation:RequestId) {
        let removed=self.take_endpoint_snapshots(endpoint,generation).await;
        self.close_snapshots(removed).await;
    }
}

#[tauri::command]
pub async fn file_manager_window(app:AppHandle,window:WebviewWindow,state:State<'_,OperatorState>,files:State<'_,FileManagerState>,code:String)->Result<String,String> {
    if window.label() != "main" {return Err("Only the main window can open file transfer".into());}
    let _account = state.account_changes.lock().await;
    let account_key = account()?;
    let code = parse_code(&code)?;
    let runtime = state.runtime().await?;
    let account_scope=runtime.file_operation_owner().await.map_err(|e|e.to_string())?;
    if account_key!=account()? {return Err("Account changed while opening file transfer".into());}
    let device = runtime.resolve_device_code(code).await.map_err(|e|e.to_string())?;
    let saved = state.local_store().await?.remembered_devices().await.map_err(|e|e.to_string())?.into_iter().find(|d|d.code == code).ok_or("Connect to the device first")?;
    let mut tab = Tab {code:code.to_string(),name:if saved.alias.is_empty(){code.to_string()}else{saved.alias},os_family:saved.os_family.map(|os|format!("{os:?}").to_lowercase()),device,generation:RequestId::new()};
    let mut windows = files.windows.lock().await;
    for (label,owner) in windows.iter() {
        if owner.account == account_key && Arc::ptr_eq(&owner.runtime,&runtime) && !owner.closed.load(Ordering::Acquire) {
            if let Some(existing) = app.get_webview_window(label) {
                let mut devices = owner.devices.lock().await;
                if devices.len() >= 16 && !devices.contains_key(&tab.code) {return Err("Open at most 16 device tabs".into());}
                if let Some(existing)=devices.get(&tab.code) && existing.device==tab.device {tab.generation=existing.generation;}
                devices.insert(tab.code.clone(),tab.clone());
                existing.emit("file-manager-open-tab",&tab).map_err(|e|e.to_string())?;
                crate::tray::restore_window(&existing).map_err(|e|e.to_string())?;
                return Ok(label.clone());
            }
        }
    }
    let local = files.local.get_or_try_init(||async {
        let paths = pab_agent_core::DataPaths::for_operator().map_err(|e|e.to_string())?;
        let database = paths.bridge_database().with_file_name("file-manager-local.sqlite3");
        pab_agent_core::ensure_data_parent(&database).map_err(|e|e.to_string())?;
        LocalFileManager::open(&database).await.map(Arc::new).map_err(|e|e.to_string())
    }).await?.clone();
    let batches=files.batches().await?;
    local.release_recorded_batch_receipts(&batches,&account_scope).await.map_err(|error|error.to_string())?;
    let label = format!("file-manager-{}",RequestId::new());
    let owner = Arc::new(Owner {scope:label.clone(),account:account_key,account_scope,runtime,local,devices:Mutex::new(HashMap::from([(tab.code.clone(),tab)])),requests:Mutex::new(HashMap::new()),snapshots:Mutex::new(Vec::new()),slots:Arc::new(Semaphore::new(8)),closed:AtomicBool::new(false),ended:watch::channel(false).0,drops:Mutex::new(VecDeque::new())});
    windows.insert(label.clone(),owner.clone());
    let builder = WebviewWindowBuilder::new(&app,&label,WebviewUrl::App("index.html?fileManager=1".into())).title("Pixels · File transfer").inner_size(1200.0,760.0).min_inner_size(960.0,640.0).decorations(false).resizable(true);
    #[cfg(all(windows, debug_assertions))]
    let builder = if let Some(arguments) = super::desktop_terminal::acceptance_browser_arguments(){builder.additional_browser_args(&arguments)}else{builder};
    match builder.build() {
        Ok(created) => {
            let close_app = app.clone();let close_label = label.clone();
            created.on_window_event(move |event| {
                if matches!(event,tauri::WindowEvent::Destroyed){let app = close_app.clone();let label = close_label.clone();tauri::async_runtime::spawn(async move {app.state::<FileManagerState>().close(&label).await;});}
                if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop {paths,position})=event {
                    let app=close_app.clone();let label=close_label.clone();let paths=paths.clone();let (x,y)=(position.x,position.y);
                    tauri::async_runtime::spawn(async move {let _=accept_native_drop(&app,&label,paths,x,y).await;});
                }
            });
            let monitor_app = app.clone();let monitor_label = label.clone();let weak = Arc::downgrade(&owner);let mut ended = owner.ended.subscribe();
            tauri::async_runtime::spawn(async move {
                let mut changes = pab_agent_core::account::account_changes();
                loop {
                    tokio::select! { _ = ended.wait_for(|v|*v) => break, changed = changes.changed() => if changed.is_err(){break;} }
                    let Some(owner) = weak.upgrade() else {break};
                    if account().ok().as_ref() != Some(&owner.account) {
                        let _ = monitor_app.emit_to(&monitor_label,"file-manager-account-ended",());
                        monitor_app.state::<FileManagerState>().close(&monitor_label).await;
                        break;
                    }
                }
            });
            Ok(label)
        },
        Err(error) => {windows.remove(&label);Err(error.to_string())}
    }
}
async fn accept_native_drop(app:&AppHandle,label:&str,paths:Vec<std::path::PathBuf>,x:f64,y:f64)->Result<(),String> {
    let window=app.get_webview_window(label).ok_or("File window is closed")?;let state=app.state::<FileManagerState>();let owner=state.owner(&window).await?;
    if paths.is_empty() || paths.len()>4096 || paths.iter().any(|path|!path.is_absolute()){return Err("Select 1..4096 local files or folders".into());}
    let native:Vec<_>=paths.iter().map(|path|pab_file_manager::encode_path(path)).collect();
    for path in &native {path.validate().map_err(str::to_owned)?;}
    let id=RequestId::new();let notice=DropNotice {drop_id:id,count:native.len(),names:paths.iter().take(20).map(|path|path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned()).collect(),x,y};
    let mut drops=owner.drops.lock().await;drops.retain(|drop|drop.created.elapsed()<Duration::from_secs(300));while drops.len()>=16 {drops.pop_front();}
    drops.push_back(NativeDrop {id,paths:native,created:Instant::now(),batch:None});drop(drops);
    window.emit("file-manager-native-drop",notice).map_err(|error|error.to_string())
}
#[tauri::command]
pub async fn file_manager_local_identity(window:WebviewWindow,state:State<'_,FileManagerState>)->Result<String,String> {
    state.owner(&window).await?;pab_os_control::execution::current_identity().map(|identity|identity.account_name).map_err(|error|error.to_string())
}
#[tauri::command]
pub async fn file_manager_reveal_local(window:WebviewWindow,state:State<'_,FileManagerState>,path:NativeFilePath)->Result<(),String> {
    state.owner(&window).await?;
    // Validate native identity through the local filesystem adapter. Never
    // interpret an escaped display label or pass a path through a shell.
    let entry=pab_file_manager::inspect(&path).await.map_err(|error|error.to_string())?;
    let native=pab_file_manager::decode_path(&path).map_err(|error|error.to_string())?;
    if !native.is_absolute(){return Err("Select an absolute local path".into());}
    let mut command;
    #[cfg(windows)]
    {command=std::process::Command::new("explorer.exe");let mut argument=std::ffi::OsString::from("/select,");argument.push(native.as_os_str());command.arg(argument);}
    #[cfg(target_os="macos")]
    {command=std::process::Command::new("/usr/bin/open");command.arg("-R").arg(&native);}
    #[cfg(not(any(windows,target_os="macos")))]
    {command=std::process::Command::new("xdg-open");command.arg(if entry.kind==pab_protocol::ManagedFileKind::Directory{native.as_path()}else{native.parent().ok_or("Path has no parent folder")?});}
    let _=entry;
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let mut child=command.spawn().map_err(|error|format!("Unable to open the local file manager: {error}"))?;
    // Reap the helper without holding a WebView command until Explorer or a
    // desktop file manager decides to exit.
    tauri::async_runtime::spawn_blocking(move || {let _=child.wait();});Ok(())
}
#[derive(Serialize)]
pub struct FileLinkStatus {connected:bool,path:&'static str,rtt_ms:Option<u64>}
#[tauri::command]
pub async fn file_manager_link_status(window:WebviewWindow,state:State<'_,FileManagerState>,code:String,force_relay:bool)->Result<FileLinkStatus,String> {
    let owner=state.owner(&window).await?;let tab=owner.devices.lock().await.get(&code).cloned().ok_or("Device does not belong to this window")?;
    Ok(match owner.runtime.file_profile_observation(tab.device,force_relay).await {
        Some((path,rtt_ms))=>FileLinkStatus {connected:true,path:match path {pab_bridge::ConnectionPath::Direct=>"p2p",pab_bridge::ConnectionPath::Relay=>"relay",pab_bridge::ConnectionPath::Unknown=>"unknown"},rtt_ms},
        None=>FileLinkStatus {connected:false,path:"unknown",rtt_ms:None},
    })
}
#[tauri::command]
pub async fn file_manager_start_drop(window:WebviewWindow,state:State<'_,FileManagerState>,id:RequestId,drop_id:RequestId,code:String,destination:NativeFilePath,execution:ExecutionSelection,conflict:pab_protocol::FileConflictPolicy,force_relay:bool)->Result<BatchView,String> {
    let owner=state.owner(&window).await?;
    let sources={let mut drops=owner.drops.lock().await;let dropped=drops.iter_mut().find(|entry|entry.id==drop_id && entry.created.elapsed()<Duration::from_secs(300)).ok_or("Native file drop expired; drag the files again")?;
        if dropped.batch.is_some_and(|batch|batch!=id){return Err("This native drop already belongs to another batch".into());}dropped.batch=Some(id);dropped.paths.clone()};
    file_manager_start_batch(window,state,id,code,pab_protocol::FileTransferDirection::Upload,sources,destination,execution,conflict,force_relay).await.map_err(|error|error.message)
}
#[tauri::command]
pub async fn file_manager_tabs(window:WebviewWindow,state:State<'_,FileManagerState>)->Result<Vec<Tab>,String> {
    Ok(state.owner(&window).await?.devices.lock().await.values().cloned().collect())
}
#[tauri::command]
pub async fn file_manager_close_tab(window:WebviewWindow,state:State<'_,FileManagerState>,code:String,generation:RequestId)->Result<(),String> {
    let owner=state.owner(&window).await?;
    {let mut devices=owner.devices.lock().await;if devices.get(&code).is_some_and(|tab|tab.generation==generation){devices.remove(&code);}}
    for query in owner.requests.lock().await.values() {if query.endpoint==code && query.tab_generation==Some(generation) && matches!(query.request.operation,Op::OpenDirectory {..}) {query.cancel.send_replace(true);}}
    // Accepted batches use their own scope and DeviceRef. Closing this browser
    // tab releases enumeration resources and never cancels those transfers.
    let snapshots=owner.take_endpoint_snapshots(&code,generation).await;
    tokio::spawn(async move {owner.close_snapshots(snapshots).await;});Ok(())
}
#[tauri::command]
pub async fn file_manager_contexts(window:WebviewWindow,state:State<'_,FileManagerState>,code:String,force_relay:bool)->Result<pab_protocol::SystemQueryReply,String> {
    let owner = state.owner(&window).await?;
    let tab = owner.devices.lock().await.get(&code).cloned().ok_or("Device does not belong to this window")?;
    let _permit = owner.slots.clone().try_acquire_owned().map_err(|_|"File window is busy")?;
    let connection = owner.runtime.file_connection(tab.device,force_relay).await.map_err(|e|e.to_string())?;
    connection.require_managed_transfer().await.map_err(|e|e.to_string())?;
    let id = RequestId::new();
    let query = pab_protocol::SystemQuery::ExecutionContexts {user:None,include_system:false,limit:1000};
    let mut reply = match connection.system_query(id,query).await {Ok(reply)=>reply,Err(original)=>connection.get_system_query(id).await.map_err(|_|format!("{original} · {id}"))?};
    let deadline = tokio::time::Instant::now()+Duration::from_secs(30);
    while matches!(reply.state.as_str(),"running"|"queued") && tokio::time::Instant::now()<deadline {
        if owner.closed.load(Ordering::Acquire) || account().ok().as_ref()!=Some(&owner.account) {return Err("File window account ended".into());}
        tokio::time::sleep(Duration::from_millis(250)).await;
        reply = connection.get_system_query(id).await.map_err(|e|format!("{e} · {id}"))?;
    }
    if let Some(pab_protocol::SystemQueryData::ExecutionContexts { backend, entries, .. }) = &mut reply.data {
        if backend == "android_app_uid" {
            super::desktop_terminal::android_app_selection(entries)?;
            entries.retain(|entry| entry.mode == pab_protocol::ExecutionMode::Service && entry.unavailable_reason.is_none());
            return Ok(reply);
        }
    }
    // Select the logged-in local console session explicitly; never pick an RDP
    // user or fall back to the service account when discovery is unavailable.
    let session_id = RequestId::new();
    let query = pab_protocol::SystemQuery::Sessions { user: None, state: None, limit: 1000 };
    let mut sessions = connection.system_query(session_id, query).await.map_err(|e|e.to_string())?;
    let deadline = tokio::time::Instant::now()+Duration::from_secs(30);
    while matches!(sessions.state.as_str(), "running"|"queued") && tokio::time::Instant::now()<deadline {
        tokio::time::sleep(Duration::from_millis(250)).await;
        sessions = connection.get_system_query(session_id).await.map_err(|e|e.to_string())?;
    }
    let Some(pab_protocol::SystemQueryData::Sessions { entries: sessions, .. }) = sessions.data else { return Err("Console session discovery unavailable".into()); };
    if let Some(pab_protocol::SystemQueryData::ExecutionContexts { entries, .. }) = &mut reply.data {
        entries.retain(|entry| entry.mode == pab_protocol::ExecutionMode::User && sessions.iter().any(|session| {
            session.active == Some(true) && session.remote == Some(false)
                && (session.session_type.as_deref() == Some("console") || session.seat.as_deref() == Some("console") || session.seat.as_deref() == Some("seat0"))
                && if entry.session_id.is_some() { entry.session_id.as_deref() == Some(session.id.as_str()) }
                   else { session.user_name.as_deref() == Some(entry.account_name.as_str()) }
        }));
    }
    Ok(reply)
}
#[tauri::command]
pub async fn file_manager_path(window:WebviewWindow,state:State<'_,FileManagerState>,path:NativeFilePath,path_style:PathStyle,name:Option<NativeFilePath>)->Result<Option<NativeFilePath>,String> {
    state.owner(&window).await?;
    match name {Some(name)=>pab_file_manager::native_join(&path,&name,path_style).map(Some),None=>pab_file_manager::native_parent(&path,path_style)}.map_err(|e|e.to_string())
}
#[tauri::command]
pub async fn file_manager_request(window:WebviewWindow,state:State<'_,FileManagerState>,endpoint:String,request_id:RequestId,operation:Op,execution:ExecutionSelection,force_relay:bool)->Result<Frame,String> {
    let owner = state.owner(&window).await?;
    let request = FileManagerRequest {expected_execution:None,schema_version:1,request_id,execution,operation};
    request.validate().map_err(str::to_owned)?;
    let tab=if endpoint=="local" {None}else{Some(owner.devices.lock().await.get(&endpoint).cloned().ok_or("Device does not belong to this window")?)};
    let device=tab.as_ref().map(|tab|tab.device);let tab_generation=tab.as_ref().map(|tab|tab.generation);
    let spec = serde_json::to_string(&(&endpoint,&request,force_relay)).map_err(|e|e.to_string())?;
    let mut requests = owner.requests.lock().await;
    if let Some(query) = requests.get(&request_id) {
        if query.spec != spec {return Err("Request ID belongs to a different operation".into());}
        return Ok(query.frame.lock().await.clone());
    }
    if requests.len() >= 4096 {
        let evict = requests.iter().filter_map(|(id,query)|query.frame.try_lock().ok().filter(|frame|matches!(frame.state.as_str(),"completed"|"failed")).map(|_|*id)).take(2048).collect::<Vec<_>>();
        for id in evict {requests.remove(&id);}
        if requests.len() >= 4096 {return Err("File window has too many unfinished requests".into());}
    }
    let permit = owner.slots.clone().try_acquire_owned().map_err(|_|"File window is busy")?;
    let frame = Frame {request_id,state:"running".into(),reply:None,message:None};
    let query = Arc::new(Query {spec,endpoint:endpoint.clone(),device,tab_generation,request:request.clone(),force_relay,cancel:watch::channel(false).0,frame:Mutex::new(frame.clone())});
    requests.insert(request_id,query.clone());drop(requests);
    tokio::spawn(async move {
        let _permit = permit;
        let result = run(&owner,&endpoint,&request,&query).await;
        let final_frame = match result {Ok(reply)=>Frame {request_id,state:if matches!(reply.result,FileManagerResult::Error {..}){"failed"}else{"completed"}.into(),reply:Some(reply),message:None},Err(message)=>Frame {request_id,state:"unconfirmed".into(),reply:None,message:Some(message)}};
        if let Some(FileManagerReply {result:FileManagerResult::Opened {snapshot_id},..}) = &final_frame.reply {
            if let Some(device)=query.device {owner.snapshots.lock().await.push(DirectorySnapshot {endpoint:endpoint.clone(),generation:query.tab_generation.expect("remote request has a tab"),device,execution:request.execution,relay:force_relay,id:*snapshot_id});}
        }
        if let Op::CloseDirectory {snapshot_id} = &request.operation {owner.snapshots.lock().await.retain(|snapshot|snapshot.endpoint!=endpoint || snapshot.relay!=force_relay || snapshot.id!=*snapshot_id);}
        *query.frame.lock().await = final_frame;
        if owner.closed.load(Ordering::Acquire) {owner.cleanup().await;}
        else if let Some(generation)=query.tab_generation {if !owner.devices.lock().await.get(&endpoint).is_some_and(|tab|tab.generation==generation){owner.cleanup_endpoint(&endpoint,generation).await;}}
    });
    Ok(frame)
}
async fn run(owner:&Owner,endpoint:&str,request:&FileManagerRequest,query:&Query)->Result<FileManagerReply,String> {
    if endpoint == "local" {return owner.local.execute(&owner.scope,request.request_id,&request.operation,query.cancel.subscribe()).await.map_err(|e|e.to_string());}
    let device=query.device.ok_or("Device identity is unavailable")?;
    let first = owner.runtime.file_manager_profile(device,request,query.force_relay).await;
    let mut reply = match first {Ok(result)=>result.reply,Err(error)=> {
        // The accepted request may have reached the target. Query its original
        // ID rather than submitting another mutation after a lost response.
        owner.runtime.get_filesystem_profile(device,request.request_id,Some(query.force_relay)).await.map_err(|_|error.to_string())?
    }};
    let mut cancellation_sent = false;
    let deadline = tokio::time::Instant::now()+Duration::from_secs(120);
    while matches!(reply.state.as_str(),"running"|"cancel_requested"|"unconfirmed") {
        if tokio::time::Instant::now() >= deadline {return Err("Remote outcome is unconfirmed; inspect the original request ID".into());}
        if *query.cancel.borrow() && reply.supports_cancel() && !cancellation_sent {
            reply = owner.runtime.cancel_filesystem_profile(device,request.request_id,Some(query.force_relay)).await.map_err(|e|e.to_string())?;
            cancellation_sent = true;
        }
        *query.frame.lock().await = Frame {request_id:request.request_id,state:reply.state.clone(),reply:None,message:reply.error.as_ref().map(|e|e.message.clone())};
        if owner.closed.load(Ordering::Acquire) && !reply.supports_cancel() {return Err("Window closed while the remote outcome remains unconfirmed".into());}
        tokio::time::sleep(Duration::from_millis(250)).await;
        reply = owner.runtime.get_filesystem_profile(device,request.request_id,Some(query.force_relay)).await.map_err(|e|e.to_string())?;
    }
    if let Some(managed) = reply.file_manager {managed.validate_for(request).map_err(str::to_owned)?;return Ok(managed);}
    Ok(FileManagerReply {schema_version:1,request_id:request.request_id,execution_context:reply.execution_context,result:FileManagerResult::Error {error:reply.error.unwrap_or_else(||pab_protocol::FileSystemError {code:reply.state,phase:"remote_files".into(),message:"The remote operation returned no file-manager result".into()})}})
}
#[tauri::command]
pub async fn file_manager_status(window:WebviewWindow,state:State<'_,FileManagerState>,request_id:RequestId)->Result<Frame,String> {
    let owner = state.owner(&window).await?;
    let query = owner.requests.lock().await.get(&request_id).cloned().ok_or("Unknown file request")?;
    let frame = query.frame.lock().await.clone();
    if frame.state == "unconfirmed" && query.endpoint == "local" {
        if let Some(managed)=owner.local.observe(&owner.scope,request_id,&query.request.operation).await.map_err(|e|e.to_string())? {
            managed.validate_for(&query.request).map_err(str::to_owned)?;
            let result=Frame {request_id,state:if matches!(managed.result,FileManagerResult::Error {..}){"failed"}else{"completed"}.into(),reply:Some(managed),message:None};
            *query.frame.lock().await=result.clone();return Ok(result);
        }
    }
    if frame.state == "unconfirmed" && query.endpoint != "local" {
        let device=query.device.ok_or("Device identity is unavailable")?;
        let fact = owner.runtime.get_filesystem_profile(device,request_id,Some(query.force_relay)).await.map_err(|e|e.to_string())?;
        if let Some(managed) = fact.file_manager {
            managed.validate_for(&query.request).map_err(str::to_owned)?;
            if let FileManagerResult::Opened {snapshot_id} = &managed.result {owner.snapshots.lock().await.push(DirectorySnapshot {endpoint:query.endpoint.clone(),generation:query.tab_generation.expect("remote request has a tab"),device,execution:query.request.execution,relay:query.force_relay,id:*snapshot_id});}
            let result = Frame {request_id,state:if matches!(managed.result,FileManagerResult::Error {..}){"failed"}else{"completed"}.into(),reply:Some(managed),message:None};
            *query.frame.lock().await = result.clone();
            if let Some(generation)=query.tab_generation {if !owner.devices.lock().await.get(&query.endpoint).is_some_and(|tab|tab.generation==generation){owner.cleanup_endpoint(&query.endpoint,generation).await;}}
            return Ok(result);
        }
    }
    Ok(frame)
}
#[tauri::command]
pub async fn file_manager_cancel(window:WebviewWindow,state:State<'_,FileManagerState>,request_id:RequestId)->Result<Frame,String> {
    let owner = state.owner(&window).await?;
    let query = owner.requests.lock().await.get(&request_id).cloned().ok_or("Unknown file request")?;
    if !matches!(query.request.operation,Op::OpenDirectory {..}) {return Err("This filesystem operation cannot be cancelled after acceptance".into());}
    query.cancel.send_replace(true);
    let mut frame = query.frame.lock().await;
    if frame.state == "running" {frame.state = "cancel_requested".into();}
    Ok(frame.clone())
}

#[derive(Serialize)]
pub struct BatchView {#[serde(flatten)] snapshot:pab_file_manager::BatchSnapshot,runner_active:bool}
#[derive(Serialize)]
pub struct BatchStartError {message:String,admitted:bool}
impl BatchStartError {
    fn rejected(error:impl std::fmt::Display)->Self {Self {message:error.to_string(),admitted:false}}
    fn unknown(error:impl std::fmt::Display)->Self {Self {message:error.to_string(),admitted:true}}
}
#[tauri::command]
pub async fn file_manager_start_batch(window:WebviewWindow,state:State<'_,FileManagerState>,id:RequestId,code:String,direction:pab_protocol::FileTransferDirection,sources:Vec<NativeFilePath>,destination:NativeFilePath,execution:ExecutionSelection,conflict:pab_protocol::FileConflictPolicy,force_relay:bool)->Result<BatchView,BatchStartError> {
    if state.exit_stopping.load(Ordering::Acquire){return Err(BatchStartError::rejected("The application is saving transfers before exit"));}
    let owner=state.owner(&window).await.map_err(BatchStartError::rejected)?;
    let tab=owner.devices.lock().await.get(&code).cloned().ok_or_else(||BatchStartError::rejected("Device does not belong to this window"))?;
    let spec=pab_protocol::FileBatchSpec {id,device:tab.device,code:parse_code(&code).map_err(BatchStartError::rejected)?,direction,sources,destination,execution,conflict,force_relay};
    spec.validate().map_err(BatchStartError::rejected)?;
    let store=state.batches().await.map_err(BatchStartError::rejected)?;
    state.prune_finished_jobs().await;
    {let jobs=state.jobs.lock().await;if jobs.len()>=128 && !jobs.contains_key(&id){return Err(BatchStartError::rejected("File task capacity reached"));}}
    // Admit the immutable submission before any remote preflight. A missing
    // WebView reply can now only observe this original ID; it cannot start a
    // duplicate preflight/worker while the first request is still preparing.
    let inserted=store.reserve_submission(&owner.account_scope,&spec).await.map_err(BatchStartError::unknown)?;
    if !inserted {
        let running=state.jobs.lock().await.get(&id).is_some_and(|job|job.owner==owner.account_scope && job.running());
        return Ok(BatchView {snapshot:store.snapshot(&owner.account_scope,id).await.map_err(BatchStartError::unknown)?,runner_active:running});
    }
    let prepared=match pab_bridge::ManagedBatchJob::prepare(owner.runtime.clone(),store.clone(),owner.local.clone(),spec,owner.account_scope.clone()).await {
        Ok((job,_))=>job,
        Err(error)=>{
            store.finish_scan(&owner.account_scope,id,Some(&error.to_string())).await.map_err(BatchStartError::unknown)?;
            return Ok(BatchView {snapshot:store.snapshot(&owner.account_scope,id).await.map_err(BatchStartError::unknown)?,runner_active:false});
        },
    };
    if account().ok().as_ref()!=Some(&owner.account) || state.exit_stopping.load(Ordering::Acquire) {
        store.set_intent(&owner.account_scope,id,pab_protocol::FileTransferIntent::Pause).await.map_err(BatchStartError::unknown)?;
        store.acknowledge_scan_stopped(&owner.account_scope,id).await.map_err(BatchStartError::unknown)?;
        return Err(BatchStartError::unknown("Account changed; inspect this batch under its original account"));
    }
    let job=state.jobs.lock().await.entry(id).or_insert(prepared).clone();
    job.launch();
    Ok(BatchView {snapshot:job.snapshot().await.map_err(BatchStartError::unknown)?,runner_active:job.running()})
}
#[tauri::command]
pub async fn file_manager_batches(window:WebviewWindow,state:State<'_,FileManagerState>)->Result<Vec<BatchView>,String> {
    let owner=state.owner(&window).await?;
    let store=state.batches().await?;
    store.recover_stale(&owner.account_scope).await.map_err(|e|e.to_string())?;
    let devices:Vec<_>=owner.devices.lock().await.values().map(|tab|tab.device).collect();
    // Apply each device scope before the history limit, so another device's
    // recent jobs cannot crowd this tab's records out of the result.
    let mut snapshots=store.list_visible(&owner.account_scope,100).await.map_err(|e|e.to_string())?;
    for device in devices {snapshots.extend(store.list_visible_for_device(&owner.account_scope,device,100).await.map_err(|e|e.to_string())?);}
    let mut seen=std::collections::HashSet::new();snapshots.retain(|row|seen.insert(row.spec.id));
    let jobs=state.jobs.lock().await;
    Ok(snapshots.into_iter().map(|snapshot|{let runner_active=jobs.get(&snapshot.spec.id).is_some_and(|job|job.owner==owner.account_scope && job.running());BatchView {snapshot,runner_active}}).collect())
}
#[tauri::command]
pub async fn file_manager_clear_completed(window:WebviewWindow,code:String,state:State<'_,FileManagerState>)->Result<Vec<String>,String> {
    let owner=state.owner(&window).await?;
    let tab=owner.devices.lock().await.get(&code).cloned().ok_or("Device does not belong to this window")?;
    state.batches().await?.clear_completed_for_device(&owner.account_scope,tab.device).await.map_err(|e|e.to_string())
}
#[tauri::command]
pub async fn file_manager_retry_failed(window:WebviewWindow,state:State<'_,FileManagerState>,id:RequestId)->Result<BatchView,String> {
    let owner=state.owner(&window).await?;let job=owned_job(&state,&owner,id).await?;
    Ok(BatchView {snapshot:job.retry_failed().await.map_err(|error|error.to_string())?,runner_active:job.running()})
}
async fn owned_job(state:&FileManagerState,owner:&Owner,id:RequestId)->Result<Arc<pab_bridge::ManagedBatchJob>,String> {
    if state.exit_stopping.load(Ordering::Acquire){return Err("The application is saving transfers before exit".into());}
    {
        let mut jobs=state.jobs.lock().await;
        if let Some(job)=jobs.get(&id) {
            if job.owner!=owner.account_scope{return Err("Batch belongs to another account".into());}
            if job.session_current(){return Ok(job.clone());}
            if job.running(){return Err("The previous account session is stopping; observe this task again shortly".into());}
            jobs.remove(&id);
        }
    }
    let store=state.batches().await?;
    let spec=store.snapshot(&owner.account_scope,id).await.map_err(|e|e.to_string())?.spec;
    let (job,_)=pab_bridge::ManagedBatchJob::prepare(owner.runtime.clone(),store,owner.local.clone(),spec,owner.account_scope.clone()).await.map_err(|e|e.to_string())?;
    Ok(state.jobs.lock().await.entry(id).or_insert(job).clone())
}
#[tauri::command]
pub async fn file_manager_batch_intent(window:WebviewWindow,state:State<'_,FileManagerState>,id:RequestId,intent:pab_protocol::FileTransferIntent)->Result<BatchView,String> {
    let owner=state.owner(&window).await?;let job=owned_job(&state,&owner,id).await?;
    let snapshot=job.request_intent(intent).await.map_err(|e|e.to_string())?;
    Ok(BatchView {snapshot,runner_active:job.running()})
}
#[tauri::command]
pub async fn file_manager_batch_items(window:WebviewWindow,state:State<'_,FileManagerState>,id:RequestId,after:u32,filter:Option<pab_protocol::FileItemState>)->Result<Vec<BatchItemView>,String> {
    let owner=state.owner(&window).await?;
    let store=state.batches().await?;let items=store.items_filtered(&owner.account_scope,id,after,filter).await.map_err(|e|e.to_string())?;
    if items.is_empty(){return Ok(Vec::new());}
    let batch=store.snapshot(&owner.account_scope,id).await.map_err(|e|e.to_string())?;
    let style=if batch.spec.direction==pab_protocol::FileTransferDirection::Download {pab_file_manager::locations().map_err(|e|e.to_string())?.path_style}else{store.batch_context(&owner.account_scope,id).await.map_err(|e|e.to_string())?.ok_or("Batch execution context is unavailable")?.path_style};
    let mut result=Vec::with_capacity(items.len());for item in items {let target=store.item_target(&owner.account_scope,item.spec.id,style).await.map_err(|e|e.to_string())?;result.push(BatchItemView {item,effective_destination:target.path});}Ok(result)
}
#[derive(Serialize)]
pub struct BatchItemView {#[serde(flatten)] item:pab_protocol::FileBatchItem,effective_destination:NativeFilePath}
#[tauri::command]
pub async fn file_manager_resolve_conflict(window:WebviewWindow,state:State<'_,FileManagerState>,batch:RequestId,item:RequestId,choice:pab_protocol::FileConflictPolicy,apply_remaining:Option<bool>)->Result<BatchView,String> {
    let owner=state.owner(&window).await?;let job=owned_job(&state,&owner,batch).await?;
    let snapshot=job.resolve_conflict_scope(item,choice,apply_remaining.unwrap_or(false)).await.map_err(|e|e.to_string())?;
    Ok(BatchView {snapshot,runner_active:job.running()})
}
