//! Durable independent file batches. User intent never manufactures a receiver
//! acknowledgement, and an uncertain original attempt is observed, not replayed.
use super::{BridgeRuntime,RemoteFileBrowser};
use super::file_batch_clock::ActiveBatchClock;
use crate::TransferControl;
use pab_file_manager::{AttemptEvidence,BatchSnapshot,BatchStore,FileBrowser,FileManagerError,ItemTarget,LocalFileBrowser,LocalFileManager};
use pab_protocol::*;
use std::{future::Future,pin::Pin,sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering}},time::Duration};
use tokio::sync::watch;

#[derive(Clone)]
enum Browser {Local(LocalFileBrowser),Remote(RemoteFileBrowser)}
impl FileBrowser for Browser {
    fn execute<'a>(&'a self,operation:FileManagerOperation,cancel:watch::Receiver<bool>)->Pin<Box<dyn Future<Output=Result<FileManagerReply,FileManagerError>>+Send+'a>> {
        match self {Self::Local(browser)=>browser.execute(operation,cancel),Self::Remote(browser)=>browser.execute(operation,cancel)}
    }
}
impl Browser {
    async fn observe(&self,id:RequestId,operation:FileManagerOperation)->Result<FileManagerResult,FileManagerError> {
        let reply=match self {Self::Local(browser)=>browser.observe_request(id,&operation).await?,Self::Remote(browser)=>browser.observe_request(id,operation).await?};
        reply.map(|reply|reply.result).ok_or(FileManagerError::Interrupted)
    }
    async fn request(&self,id:RequestId,operation:FileManagerOperation)->Result<FileManagerResult,FileManagerError> {
        let (_send,cancel)=watch::channel(false);
        let reply=match self {Self::Local(browser)=>browser.execute_request(id,operation,cancel).await?,Self::Remote(browser)=>browser.execute_request(id,operation,cancel).await?};
        Ok(reply.result)
    }
    async fn inspect(&self,path:&NativeFilePath)->Result<Option<ManagedFileEntry>,FileManagerError> {
        match self.request(RequestId::new(),FileManagerOperation::Inspect {path:path.clone()}).await? {
            FileManagerResult::Entry {entry}=>Ok(Some(entry)),
            FileManagerResult::Error {error} if error.code=="not_found"=>Ok(None),
            FileManagerResult::Error {error}=>Err(error_message(error)),
            _=>Err(FileManagerError::Invalid("missing file metadata")),
        }
    }
    async fn path_identity(&self,path:&NativeFilePath)->Result<FilePathIdentity,FileManagerError> {
        match self.request(RequestId::new(),FileManagerOperation::InspectPathIdentity {path:path.clone()}).await? {
            FileManagerResult::PathIdentity {identity,..}=>Ok(identity),
            FileManagerResult::Error {error}=>Err(error_message(error)),
            _=>Err(FileManagerError::Invalid("missing filesystem object observation")),
        }
    }
}
fn error_message(error:FileSystemError)->FileManagerError {FileManagerError::Io(std::io::Error::other(format!("{}: {}",error.code,error.message)))}
fn io_error(error:impl std::fmt::Display)->FileManagerError {FileManagerError::Io(std::io::Error::other(error.to_string()))}
fn terminal(phase:&str)->bool {matches!(phase,"completed"|"completed_with_skips"|"partial_failed"|"cancelled"|"failed")}

pub struct ManagedBatchJob {
    pub spec:FileBatchSpec,
    pub owner:String,
    runtime:Arc<BridgeRuntime>,store:BatchStore,local:Arc<LocalFileManager>,
    source:Browser,destination:Browser,source_style:PathStyle,destination_style:PathStyle,
    same_filesystem_scope:Option<String>,
    expected:ExecutionContext,worker:RequestId,
    intent:watch::Sender<FileTransferIntent>,running:AtomicBool,halted:AtomicBool,
    account_revision:u64,revoked:AtomicBool,
    coordinator_failure:std::sync::Mutex<Option<String>>,
    clock:Arc<ActiveBatchClock>,
    #[cfg(test)]
    publication_barrier:std::sync::Mutex<Option<crate::connection::PublicationBarrier>>,
    #[cfg(test)]
    publication_writer_delay:std::sync::Mutex<Option<sqlx::SqlitePool>>,
}
impl ManagedBatchJob {
    /// Returns `inserted=false` for an original accepted ID. A caller must not
    /// launch a second worker merely because its submission response was lost.
    pub async fn prepare(runtime:Arc<BridgeRuntime>,store:BatchStore,local:Arc<LocalFileManager>,spec:FileBatchSpec,owner:String)->Result<(Arc<Self>,bool),FileManagerError> {
        spec.validate().map_err(FileManagerError::Invalid)?;
        if runtime.file_operation_owner().await.map_err(io_error)?!=owner {return Err(FileManagerError::NotFound);}
        match store.snapshot(&owner,spec.id).await {
            Ok(original) if original.spec!=spec=>return Err(FileManagerError::Conflict),
            Ok(_)=>{}, // Accepted work follows its frozen DeviceRef, even if its display code changed.
            Err(FileManagerError::NotFound)=>{
                let reference=runtime.resolve_cached_device_code(spec.code).await.map_err(io_error)?;
                if reference!=spec.device {return Err(FileManagerError::Invalid("device code identifies a different device"));}
            },
            Err(error)=>return Err(error),
        }
        let connection=runtime.file_connection(spec.device,spec.force_relay).await.map_err(io_error)?;
        connection.require_managed_transfer().await.map_err(io_error)?;
        let mut remote=RemoteFileBrowser::owned(runtime.clone(),spec.device,spec.execution,spec.force_relay,owner.clone());
        if let Some(frozen)=store.batch_context(&owner,spec.id).await.or_else(|error|if matches!(error,FileManagerError::NotFound){Ok(None)}else{Err(error)})? {
            remote=remote.with_expected_context(frozen);
        }
        let (_send,cancel)=watch::channel(false);
        let locations=remote.execute(FileManagerOperation::Locations {},cancel).await?;
        let expected=locations.execution_context.ok_or(FileManagerError::Invalid("remote execution identity is unavailable"))?;
        FileExecutionIdentity::from_context(&expected).map_err(FileManagerError::Invalid)?;
        let FileManagerResult::Locations {locations:remote_locations}=locations.result else {return Err(FileManagerError::Invalid("remote locations are unavailable"));};
        let local_locations=pab_file_manager::locations()?;
        let local_scope=local_locations.filesystem_scope.as_ref().ok_or(FileManagerError::Invalid("local filesystem scope unavailable"))?;
        let remote_scope=remote_locations.filesystem_scope.as_ref().ok_or(FileManagerError::Invalid("remote filesystem scope unavailable; update the device"))?;
        let same_filesystem_scope=(local_scope==remote_scope).then(||local_scope.clone());
        let local_style=local_locations.path_style;
        let local_browser=Browser::Local(local.browser(format!("batch:{}",spec.id)));
        let remote_browser=Browser::Remote(remote.with_expected_context(expected.clone()));
        let (source,destination,source_style,destination_style)=match spec.direction {
            FileTransferDirection::Upload=>(local_browser,remote_browser,local_style,remote_locations.path_style),
            FileTransferDirection::Download=>(remote_browser,local_browser,remote_locations.path_style,local_style),
        };
        let target=destination.inspect(&spec.destination).await?.ok_or(FileManagerError::Invalid("destination directory does not exist"))?;
        if target.kind!=ManagedFileKind::Directory {return Err(FileManagerError::Invalid("select a destination directory"));}
        if runtime.file_operation_owner().await.map_err(io_error)?!=owner {return Err(FileManagerError::NotFound);}
        let identity=destination.path_identity(&spec.destination).await?;
        if identity.kind!=Some(ManagedFileKind::Directory){return Err(FileManagerError::Invalid("destination directory changed"));}
        let destination_ready=pab_file_manager::DirectoryReady {path:spec.destination.clone(),identity:identity.object.ok_or(FileManagerError::Invalid("destination directory identity unavailable"))?};
        let inserted=store.create_with_destination(&owner,&spec,&destination_ready).await?;
        let account_revision=runtime.inner.account_store.read().map_err(io_error)?.revision;
        store.freeze_batch_context(&owner,spec.id,&expected).await?;
        local.browser(format!("batch:{}",spec.id)).release_recorded_receipts(&store,&owner,spec.id).await?;
        let intent=store.snapshot(&owner,spec.id).await?.intent;
        Ok((Arc::new(Self {spec,owner,runtime,store,local,source,destination,source_style,destination_style,same_filesystem_scope,expected,worker:RequestId::new(),intent:watch::channel(intent).0,running:AtomicBool::new(false),halted:AtomicBool::new(false),account_revision,revoked:AtomicBool::new(false),coordinator_failure:std::sync::Mutex::new(None),clock:Arc::new(ActiveBatchClock::default()),#[cfg(test)] publication_barrier:std::sync::Mutex::new(None),#[cfg(test)] publication_writer_delay:std::sync::Mutex::new(None)}),inserted))
    }
    #[cfg(test)]
    pub(super) fn set_publication_barrier(&self,barrier:crate::connection::PublicationBarrier){*self.publication_barrier.lock().unwrap()=Some(barrier);}
    #[cfg(test)]
    pub(super) fn delay_publication_writer_for_test(&self,pool:sqlx::SqlitePool){*self.publication_writer_delay.lock().unwrap()=Some(pool);}
    #[cfg(test)]
    pub(super) async fn observe_once_for_test(&self,id:RequestId)->Result<(),FileManagerError> {
        self.store.heartbeat(&self.owner,self.worker).await?;
        let item=self.store.claim_for_observation(&self.owner,id,self.worker).await?;self.observe_item(item).await
    }
    pub fn running(&self)->bool {self.running.load(Ordering::Acquire)}
    pub fn session_current(&self)->bool {self.account_current()}
    /// Stop this coordinator during an explicitly confirmed process shutdown.
    /// Outstanding native/remote effects remain reconciling until proven; this
    /// cannot manufacture a receiver pause or reactivate an old coordinator.
    pub fn request_shutdown(&self){self.clock.set_enabled(false);self.revoked.store(true,Ordering::Release);}
    pub async fn retry_failed(self:&Arc<Self>)->Result<BatchSnapshot,FileManagerError> {
        if !self.account_current(){return Err(FileManagerError::NotFound);}
        if self.runtime.file_operation_owner().await.map_err(io_error)?!=self.owner {return Err(FileManagerError::NotFound);}
        self.store.verify_runner_control(&self.owner,self.spec.id,self.worker).await?;
        if self.running(){return Err(FileManagerError::Conflict);}
        if self.store.retry_failed(&self.owner,self.spec.id).await?==0 {return Err(FileManagerError::Invalid("there are no failed files to retry"));}
        self.intent.send_replace(FileTransferIntent::Run);self.halted.store(false,Ordering::Release);self.launch();self.snapshot().await
    }
    pub async fn resolve_conflict(self:&Arc<Self>,item:RequestId,choice:FileConflictPolicy)->Result<BatchSnapshot,FileManagerError> {return self.resolve_conflict_scope(item,choice,false).await;}
    pub async fn resolve_conflict_scope(self:&Arc<Self>,item:RequestId,choice:FileConflictPolicy,apply_remaining:bool)->Result<BatchSnapshot,FileManagerError> {
        if !self.account_current(){return Err(FileManagerError::NotFound);}
        if self.runtime.file_operation_owner().await.map_err(io_error)?!=self.owner {return Err(FileManagerError::NotFound);}
        self.store.verify_runner_control(&self.owner,self.spec.id,self.worker).await?;
        self.store.resolve_conflict_scope(&self.owner,self.spec.id,item,choice,apply_remaining).await?;self.launch();self.snapshot().await
    }
    pub async fn snapshot(&self)->Result<BatchSnapshot,FileManagerError> {self.store.snapshot(&self.owner,self.spec.id).await}
    pub async fn items(&self,after:u32)->Result<Vec<FileBatchItem>,FileManagerError> {self.store.items(&self.owner,self.spec.id,after,128).await}
    pub async fn request_intent(self:&Arc<Self>,intent:FileTransferIntent)->Result<BatchSnapshot,FileManagerError> {
        if !self.account_current(){return Err(FileManagerError::NotFound);}
        if self.runtime.file_operation_owner().await.map_err(io_error)?!=self.owner {return Err(FileManagerError::NotFound);}
        self.store.verify_runner_control(&self.owner,self.spec.id,self.worker).await?;
        let snapshot=self.snapshot().await?;
        if terminal(&snapshot.phase) {return Ok(snapshot);}
        if intent==FileTransferIntent::Run && snapshot.intent==FileTransferIntent::Pause && snapshot.phase!="paused" {return Err(FileManagerError::Invalid("wait for pause acknowledgement before resuming"));}
        self.store.set_intent(&self.owner,self.spec.id,intent).await?;
        self.clock.set_enabled(intent==FileTransferIntent::Run);
        self.intent.send_replace(intent);
        if intent==FileTransferIntent::Run {self.halted.store(false,Ordering::Release);self.launch();}
        self.snapshot().await
    }
    pub fn launch(self:&Arc<Self>) {
        if self.running.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return;}
        let job=self.clone();
        tokio::spawn(async move {
            loop {
                if let Err(error)=Box::pin(job.guarded_round()).await {
                    job.halted.store(true,Ordering::Release);
                    #[cfg(test)]
                    eprintln!("file batch {} coordinator stopped: {error:?}",job.spec.id);
                    tracing::warn!(batch=%job.spec.id,%error,"file batch stopped; original attempts remain observable");
                    if let Err(retire_error)=job.store.retire_worker_with_reason(&job.owner,job.spec.id,job.worker,Some(&error.to_string())).await {
                        if !matches!(retire_error,FileManagerError::NotFound){tracing::warn!(batch=%job.spec.id,%retire_error,"file batch lease retirement requires later reconciliation");}
                    }
                }
                if let Err(error)=job.local.browser(format!("batch:{}",job.spec.id)).release_recorded_receipts(&job.store,&job.owner,job.spec.id).await {
                    tracing::warn!(batch=%job.spec.id,%error,"recorded local mutation receipts remain retained; acknowledgement will retry on reopen");
                }
                if job.snapshot().await.is_ok_and(|snapshot|terminal(&snapshot.phase)) {let _=job.local.release(&format!("batch:{}",job.spec.id)).await;}
                job.running.store(false,Ordering::Release);
                if job.halted.load(Ordering::Acquire){break;}
                let pending=job.snapshot().await.is_ok_and(|snapshot|snapshot.phase=="queued" && snapshot.intent==FileTransferIntent::Run);
                if !pending || job.running.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){break;}
            }
        });
    }
    fn account_current(&self)->bool {
        if self.revoked.load(Ordering::Acquire) || *self.runtime.inner.shutdown.borrow(){return false;}
        self.runtime.inner.account_store.read().is_ok_and(|state|state.revision==self.account_revision && state.user.is_some_and(|user|self.runtime.inner.store.account_origin.as_ref().is_some_and(|origin|format!("{origin}\n{}",user.id)==self.owner)))
    }
    async fn guarded_round(&self)->Result<(),FileManagerError> {
        let mut round=Box::pin(self.round());
        let ended=async {loop {if !self.account_current(){return;}tokio::time::sleep(Duration::from_millis(250)).await;}};
        tokio::pin!(ended);
        let finished=tokio::select! {biased;_=&mut ended=>None,result=&mut round=>Some(result)};
        if finished.is_some() && self.account_current(){return finished.unwrap();}
        self.revoked.store(true,Ordering::Release);self.halted.store(true,Ordering::Release);
        self.clock.set_enabled(false);
        let pause=async {
            let snapshot=self.snapshot().await?;
            let intent=if snapshot.intent==FileTransferIntent::Cancel{FileTransferIntent::Cancel}else{FileTransferIntent::Pause};
            self.store.set_intent(&self.owner,self.spec.id,intent).await?;self.intent.send_replace(intent);
            Ok::<(),FileManagerError>(())
        };
        // The worker may hold the sole SQLite writer. Poll shutdown intent
        // and the worker together; waiting on SQL while leaving round unpolled
        // deadlocks. Timeout owns (and drops) round, including its transaction.
        if finished.is_none() {
            let drain=async move {let _=tokio::time::timeout(Duration::from_secs(3),round).await;};
            let (_,result)=tokio::join!(drain,pause);result?;
        } else {drop(round);pause.await?;}
        // Platform IO already in flight retains its native staging lock until
        // it actually exits; relinquishing the coordinator proves no pause.
        let failure=self.coordinator_failure.lock().unwrap_or_else(|error|error.into_inner()).clone();
        self.store.retire_worker_with_reason(&self.owner,self.spec.id,self.worker,failure.as_deref()).await?;
        if !self.snapshot().await?.scan_complete {let _=self.store.acknowledge_scan_stopped(&self.owner,self.spec.id).await;}
        let _=self.local.release(&format!("batch:{}",self.spec.id)).await;
        match failure {Some(reason)=>Err(io_error(reason)),None=>Ok(())}
    }
    async fn round(&self)->Result<(),FileManagerError> {
        if !self.account_current(){return Err(FileManagerError::NotFound);}
        self.store.heartbeat(&self.owner,self.worker).await?;
        self.store.recover_stale(&self.owner).await?;
        self.store.claim_runner(&self.owner,self.spec.id,self.worker).await?;
        let token=self.store.begin_elapsed(&self.owner,self.spec.id,self.worker).await?;
        self.clock.reset(*self.intent.borrow()==FileTransferIntent::Run && self.account_current());
        let mut work=Box::pin(self.round_work());
        let mut heartbeat=Box::pin(async {
            let mut tick=tokio::time::interval(Duration::from_secs(1));tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                if !self.account_current(){self.clock.set_enabled(false);}
                self.store.heartbeat(&self.owner,self.worker).await?;
                self.store.checkpoint_elapsed(&self.owner,self.spec.id,self.worker,token,FileByteCount(self.clock.milliseconds())).await?;
            }
            #[allow(unreachable_code)] Ok::<(),FileManagerError>(())
        });
        // Poll the worker while checkpointing: it may own the sole SQLite
        // connection. Drop both futures before final SQL, including on error.
        let result=tokio::select! {result=&mut work=>result,result=&mut heartbeat=>result};
        self.clock.set_enabled(false);
        drop(work);drop(heartbeat);
        let checkpoint=self.store.checkpoint_elapsed(&self.owner,self.spec.id,self.worker,token,FileByteCount(self.clock.milliseconds())).await;
        if let Err(error)=checkpoint {
            if result.is_ok(){return Err(error);}
            tracing::warn!(batch=%self.spec.id,%error,"final active time checkpoint requires reconciliation");
        }
        if result.is_ok(){self.store.release_runner(&self.owner,self.spec.id,self.worker).await?;}
        result
    }
    async fn round_work(&self)->Result<(),FileManagerError> {
        let snapshot=self.snapshot().await?;
        if !snapshot.scan_complete {
            if snapshot.intent!=FileTransferIntent::Run {self.store.acknowledge_scan_stopped(&self.owner,self.spec.id).await?;return Ok(());}
            let _activity=self.clock.enter();
            let (stop,cancel)=watch::channel(false);let mut intent=self.intent.subscribe();
            let scan=pab_file_manager::plan_tree(&self.store,&self.owner,&self.spec,&self.source,self.source_style,self.destination_style,cancel);
            tokio::pin!(scan);
            let result=loop {tokio::select! {
                result=&mut scan=>break result,
                changed=intent.changed()=>{if changed.is_err() || *intent.borrow()!=FileTransferIntent::Run {stop.send_replace(true);}},
            }};
            if let Err(error)=result {
                if self.store.scan_intent(&self.owner,self.spec.id).await?!=FileTransferIntent::Run {self.store.acknowledge_scan_stopped(&self.owner,self.spec.id).await?;return Ok(());}
                self.store.finish_scan(&self.owner,self.spec.id,Some(&error.to_string())).await?;return Ok(());
            }
        }
        let worker=||async {
            let result=self.worker_loop().await;
            // Let the sibling observe revocation and cancel its receiver.
            // try_join! would drop that receiver before it could acknowledge
            // cancellation; guarded_round bounds draining both workers.
            if let Err(error)=&result {
                if self.account_current(){self.coordinator_failure.lock().unwrap_or_else(|error|error.into_inner()).get_or_insert_with(||error.to_string());}
                self.revoked.store(true,Ordering::Release);
            }
            result
        };
        let (first,second)=tokio::join!(worker(),worker());first?;second
    }
    async fn worker_loop(&self)->Result<(),FileManagerError> {
        loop {
            if self.revoked.load(Ordering::Acquire){return Err(FileManagerError::NotFound);}
            self.store.propagate_blocked_children(&self.owner,self.spec.id).await?;
            if let Some(item)=self.store.claim(&self.owner,self.spec.id,self.worker).await? {
                let result={let _activity=self.clock.enter();Box::pin(self.run_item(item.clone())).await};
                if let Err(error)=result {
                    let current=self.store.item(&self.owner,item.spec.id).await?;
                    if !current.state.terminal() && current.state!=FileItemState::WaitingConflict {
                        if current.state==FileItemState::Preparing {self.advance(current.spec.id,FileItemState::Failed,current.confirmed.0,Some(&error.to_string())).await?;}
                        else {self.advance(current.spec.id,FileItemState::Reconciling,current.confirmed.0,Some(&error.to_string())).await?;self.store.release_for_observation(&self.owner,current.spec.id,self.worker).await?;}
                    }
                }
                continue;
            }
            let snapshot=self.snapshot().await?;
            if terminal(&snapshot.phase) || matches!(snapshot.phase.as_str(),"paused"|"waiting_conflict") {return Ok(());}
            let observations=self.store.pending_observations(&self.owner,self.spec.id).await?;
            if let Some(item)=observations.first() {
                if let Ok(item)=self.store.claim_for_observation(&self.owner,item.spec.id,self.worker).await {
                    let observed=Box::pin(self.observe_item(item.clone())).await;
                    if observed.is_err() {let _=self.store.release_for_observation(&self.owner,item.spec.id,self.worker).await;}
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            } else {tokio::time::sleep(Duration::from_millis(250)).await;}
        }
    }
    async fn advance(&self,id:RequestId,next:FileItemState,confirmed:u64,message:Option<&str>)->Result<FileBatchItem,FileManagerError> {
        // Intent commands can advance the revision while a worker is waiting
        // for the receiver. Retry the CAS, never the filesystem operation.
        for _ in 0..8 {
            let current=self.store.item(&self.owner,id).await?;
            match self.store.advance(&self.owner,id,self.worker,current.revision,next,FileByteCount(confirmed.max(current.confirmed.0)),message).await {
                Err(FileManagerError::Conflict)=>continue,
                result=>return result,
            }
        }
        Err(FileManagerError::Conflict)
    }
    async fn complete(&self,id:RequestId,size:u64)->Result<(),FileManagerError> {
        let mut current=self.store.item(&self.owner,id).await?;
        if matches!(current.state,FileItemState::Preparing|FileItemState::Transferring) {current=self.advance(id,FileItemState::Verifying,size,None).await?;}
        if matches!(current.state,FileItemState::Verifying|FileItemState::PauseRequested|FileItemState::CancelRequested) {self.advance(id,FileItemState::Publishing,size,None).await?;}
        self.advance(id,FileItemState::Completed,size,None).await?;Ok(())
    }
    async fn stop_item(&self,id:RequestId,confirmed:u64)->Result<(),FileManagerError> {
        // The SQL admission gate can see a stop before its watch notification.
        // Do not turn that already-persisted stop back into Run using stale UI state.
        let intent=self.store.snapshot(&self.owner,self.spec.id).await?.intent;
        let next=if intent==FileTransferIntent::Cancel {FileItemState::Cancelled}else{FileItemState::Paused};
        self.advance(id,next,confirmed,None).await?;
        if intent==FileTransferIntent::Run {self.store.set_intent(&self.owner,self.spec.id,FileTransferIntent::Run).await?;}
        Ok(())
    }
    async fn run_item(&self,item:FileBatchItem)->Result<(),FileManagerError> {
        if self.runtime.file_operation_owner().await.map_err(io_error)?!=self.owner {return Err(FileManagerError::NotFound);}
        if *self.intent.borrow()!=FileTransferIntent::Run {return self.stop_item(item.spec.id,item.confirmed.0).await;}
        if item.spec.kind==ManagedFileKind::Directory && self.store.directory_ready(&self.owner,item.spec.id).await?.is_some() {
            return self.finalize_directory(item).await;
        }
        let current=self.source.inspect(&item.spec.source).await?;
        let Some(current)=current else {self.advance(item.spec.id,FileItemState::Failed,item.confirmed.0,Some("Source no longer exists")).await?;return Ok(());};
        if current.kind!=item.spec.kind || current.kind==ManagedFileKind::File && (current.size!=Some(item.spec.size) || current.modified_at_unix_ms!=item.spec.modified_at_unix_ms) {
            self.advance(item.spec.id,FileItemState::Failed,item.confirmed.0,Some("Source changed after it was selected")).await?;return Ok(());
        }
        let mut target=self.store.item_target(&self.owner,item.spec.id,self.destination_style).await?;
        if let Some(scope)=&self.same_filesystem_scope && matches!(item.spec.kind,ManagedFileKind::File|ManagedFileKind::Directory) {
            // Comparing path spelling across devices would reject unrelated
            // C:\\Users or /home trees. Only compare physical objects after the
            // two adapters reported the same native filesystem namespace.
            let source=self.source.path_identity(&item.spec.source).await?;
            let destination=self.destination.path_identity(&target.path).await?;
            if &source.filesystem_scope!=scope || &destination.filesystem_scope!=scope {return Err(FileManagerError::Invalid("filesystem scope changed; reopen the file connection"));}
            source.validate_copy_destination(&destination).map_err(FileManagerError::Invalid)?;
        }
        let previous=match item.previous_attempt {Some(id)=>Some(self.store.attempt_evidence(&self.owner,id).await?),None=>None};
        let resume=previous.as_ref().is_some_and(|evidence|!evidence.local_staging_discarded && evidence.receipt.as_ref().is_some_and(|receipt|matches!(receipt.state.as_str(),"paused"|"failed"|"interrupted"|"cancelled") || self.spec.direction==FileTransferDirection::Download && receipt.state=="completed" && evidence.local_checkpoint.is_some()) && evidence.digest.is_some());
        if !resume {
            if let Some(existing)=self.destination.inspect(&target.path).await? {
                if item.spec.kind==ManagedFileKind::Directory && existing.kind==ManagedFileKind::Directory && matches!(target.choice,FileConflictPolicy::Ask|FileConflictPolicy::Replace) {
                    let identity=self.destination.path_identity(&target.path).await?;
                    if identity.kind!=Some(ManagedFileKind::Directory) {return Err(FileManagerError::Conflict);}
                    let existing=identity.object.ok_or(FileManagerError::Invalid("existing directory identity unavailable"))?;
                    self.store.save_item_target(&self.owner,item.spec.id,self.worker,&target).await?;
                    return self.prepare_directory(&item,&target.path,Some(existing)).await;
                }
                match target.choice {
                    FileConflictPolicy::Skip=>{self.advance(item.spec.id,FileItemState::Skipped,0,Some("Skipped by conflict policy")).await?;return Ok(());},
                    FileConflictPolicy::Ask=>{self.advance(item.spec.id,FileItemState::WaitingConflict,0,Some("Destination already exists")).await?;return Ok(());},
                    FileConflictPolicy::Replace if item.spec.kind==ManagedFileKind::File && existing.kind==ManagedFileKind::File=>target.overwrite=true,
                    FileConflictPolicy::Replace=>{self.advance(item.spec.id,FileItemState::WaitingConflict,0,Some("Different object types require skip or keep both")).await?;return Ok(());},
                    FileConflictPolicy::KeepBoth=>{
                        let original=target.path.clone();let mut found=false;
                        for number in 1..=9999 {let candidate=pab_file_manager::keep_both_candidate(&original,self.destination_style,number)?;if self.destination.inspect(&candidate).await?.is_none(){target.path=candidate;found=true;break;}}
                        if !found {return Err(FileManagerError::Capacity);}
                    },
                }
            }
        }
        self.store.save_item_target(&self.owner,item.spec.id,self.worker,&target).await?;
        if item.spec.kind==ManagedFileKind::Directory {
            return self.prepare_directory(&item,&target.path,None).await;
        }
        if item.spec.kind!=ManagedFileKind::File {self.advance(item.spec.id,FileItemState::Skipped,0,Some("Unsupported filesystem object")).await?;return Ok(());}
        self.transfer_item(item,target,previous.filter(|_|resume)).await
    }
    async fn prepare_directory(&self,item:&FileBatchItem,path:&NativeFilePath,existing:Option<String>)->Result<(),FileManagerError> {
        let Some(preparation)=self.store.begin_directory_preparation(&self.owner,item.spec.id,self.worker,path,self.destination_style,existing).await? else {return self.stop_item(item.spec.id,0).await;};
        let result=self.destination.request(preparation.request_id,preparation.operation).await?;
        self.finish_preparation(item,result).await
    }
    async fn finish_preparation(&self,item:&FileBatchItem,result:FileManagerResult)->Result<(),FileManagerError> {
        let attempt=item.attempt.ok_or(FileManagerError::Conflict)?;
        self.store.record_directory_preparation(&self.owner,item.spec.id,self.worker,attempt,&result).await?;
        self.acknowledge_local_directory_receipt(attempt).await;
        match result {
            FileManagerResult::DirectoryPrepared {..}=>Ok(()),
            FileManagerResult::Error {error}=>{self.advance(item.spec.id,FileItemState::Failed,0,Some(&error.message)).await?;Ok(())},
            _=>Err(FileManagerError::Conflict),
        }
    }
    async fn finalize_directory(&self,item:FileBatchItem)->Result<(),FileManagerError> {
        let Some(finalization)=self.store.begin_directory_finalization(&self.owner,item.spec.id,self.worker).await? else {return self.stop_item(item.spec.id,0).await;};
        let result=self.destination.request(finalization.request_id,finalization.operation).await?;
        self.finish_directory(&item,result).await
    }
    async fn finish_directory(&self,item:&FileBatchItem,result:FileManagerResult)->Result<(),FileManagerError> {
        let attempt=item.attempt.ok_or(FileManagerError::Conflict)?;
        self.store.record_directory_outcome(&self.owner,item.spec.id,self.worker,attempt,&result).await?;
        self.acknowledge_local_directory_receipt(attempt).await;
        match result {
            FileManagerResult::DirectoryMetadata {..}=>self.complete(item.spec.id,0).await,
            FileManagerResult::Error {error}=>{self.advance(item.spec.id,FileItemState::Failed,0,Some(&error.message)).await?;Ok(())},
            _=>Err(FileManagerError::Conflict),
        }
    }
    async fn acknowledge_local_directory_receipt(&self,attempt:RequestId) {
        let Browser::Local(browser)=&self.destination else {return};
        let result=async {
            let receipt=self.store.recorded_directory_receipt(&self.owner,self.spec.id,attempt).await?.ok_or(FileManagerError::Conflict)?;
            browser.acknowledge_receipt(&receipt).await?;
            Ok::<(),FileManagerError>(())
        }.await;
        // The batch fact is already durable. A failed cleanup ACK cannot turn
        // that known outcome into failure; retain the receipt and retry later.
        if let Err(error)=result {tracing::warn!(batch=%self.spec.id,%attempt,%error,"local mutation receipt acknowledgement deferred");}
    }
    async fn transfer_item(&self,item:FileBatchItem,target:ItemTarget,previous:Option<AttemptEvidence>)->Result<(),FileManagerError> {
        let id=item.attempt.ok_or(FileManagerError::Conflict)?;
        let control=TransferControl::default();control.expect_context(self.expected.clone());control.expect_size(item.spec.size.0);control.expect_source_modified(item.spec.modified_at_unix_ms);
        control.set_resume_staging(previous.as_ref().and_then(|evidence|evidence.local_staging.clone()));
        if self.spec.direction==FileTransferDirection::Download {
            let store=self.store.clone();let owner=self.owner.clone();let worker=self.worker;let item_id=item.spec.id;
            #[cfg(test)]
            let delay=self.publication_writer_delay.lock().unwrap().clone();
            control.set_publication_guard(Arc::new(move |evidence| {
                let store=store.clone();let owner=owner.clone();
                #[cfg(test)]
                let delay=delay.clone();
                Box::pin(async move {
                    #[cfg(test)]
                    if let Some(pool)=delay {
                        let tx=pab_file_manager::begin_write_transaction(&pool,"UPDATE fm_batches SET revision=revision WHERE 0").await.map_err(|error|crate::BridgeError::FileTransfer(error.to_string()))?;
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        tx.commit().await.map_err(|error|crate::BridgeError::FileTransfer(error.to_string()))?;
                    }
                    store.observe_attempt(&owner,item_id,worker,id,&evidence).await.map_err(|error|crate::BridgeError::FileTransfer(error.to_string()))
                })
            }));
        }
        #[cfg(test)]
        if let Some(barrier)=self.publication_barrier.lock().unwrap().clone(){control.set_publication_barrier(barrier);}
        let connection=self.runtime.file_connection(self.spec.device,self.spec.force_relay).await.map_err(io_error)?;
        connection.require_managed_transfer().await.map_err(io_error)?;
        let expected=FileExecutionIdentity::from_context(&self.expected).map_err(FileManagerError::Invalid)?;
        let execution=connection.resolve_file_execution(&expected).await.map_err(io_error)?;
        let parent=self.store.item_destination_parent(&self.owner,item.spec.id,&target.path,self.destination_style).await?;
        self.runtime.begin_managed_attempt(&self.owner,id,self.spec.device,if self.spec.direction==FileTransferDirection::Upload{"upload"}else{"download"},&item.spec.source,&target.path,target.overwrite).await.map_err(io_error)?;
        let options=FileTransferOptions { destination_parent:Some(parent),
            source_constraint:Some(FileTransferSourceConstraint {size:item.spec.size,modified_at_unix_ms:item.spec.modified_at_unix_ms}),
            metadata:Some(FileTransferMetadata {modified_at_unix_ms:item.spec.modified_at_unix_ms,unix_mode:item.spec.unix_mode}),
            expected_execution:Some(expected),execution,resume_from:previous.as_ref().and(item.previous_attempt)
        };
        self.advance(item.spec.id,FileItemState::Transferring,item.confirmed.0,None).await?;
        let local_path=pab_file_manager::decode_path(if self.spec.direction==FileTransferDirection::Upload {&item.spec.source}else{&target.path})?;
        let offset=Arc::new(AtomicU64::new(item.confirmed.0));let count=offset.clone();
        let progress=move |bytes:u64,_size:u64|{count.store(bytes,Ordering::Release);};
        let mut operation=Box::pin(async {
            if self.spec.direction==FileTransferDirection::Upload {connection.upload_native_file_with_options(id,&local_path,&target.path,target.overwrite,&options,&control,progress).await}
            else {connection.download_native_file_with_options(id,&item.spec.source,&local_path,target.overwrite,&options,previous.as_ref().and_then(|evidence|evidence.digest.as_deref()),&control,progress).await}
        });
        let mut tick=tokio::time::interval(Duration::from_millis(250));tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut stop_sent=None;let mut last=item.confirmed.0;
        let outcome={
            let monitor=async {
                loop {
                    tick.tick().await;
                if self.revoked.load(Ordering::Acquire){control.cancel();continue;}
                let intent=*self.intent.borrow();
                if intent!=FileTransferIntent::Run && stop_sent!=Some(intent) {
                    if !control.remote_requested() {if intent==FileTransferIntent::Cancel {control.cancel_and_discard();}else{control.cancel();}}
                    else {match connection.stop_transfer(id,intent).await {Ok(snapshot)=>{self.observe_receipt(&item,&control,snapshot).await?;if self.spec.direction==FileTransferDirection::Download && !control.committing(){if intent==FileTransferIntent::Cancel {control.cancel_and_discard();}else{control.cancel();}}stop_sent=Some(intent);},Err(error)=>tracing::debug!(%id,%error,"waiting for receiver stop acknowledgement")}}
                }
                self.save_control(&item,&control).await?;
                let bytes=offset.load(Ordering::Acquire).max(last).min(item.spec.size.0);
                if bytes!=last {
                    let current=self.store.item(&self.owner,item.spec.id).await?;
                    let phase=if current.state==FileItemState::Preparing {FileItemState::Transferring}else{current.state};
                    self.advance(item.spec.id,phase,bytes,None).await?;
                    self.runtime.managed_attempt_progress(id,bytes,item.spec.size.0).await.map_err(io_error)?;last=bytes;
                }
                }
                #[allow(unreachable_code)] Ok::<(),FileManagerError>(())
            };
            tokio::pin!(monitor);
            // The data future's durable publication guard uses the same task
            // database as progress. Keep polling it while monitor waits for
            // that writer, rather than awaiting SQL inside a select branch.
            tokio::select! {biased;
                result=&mut operation=>result,
                result=&mut monitor=>{control.cancel();Err(crate::BridgeError::FileTransfer(result.err().map_or_else(||"transfer monitor stopped".into(),|error|error.to_string())))},
            }
        };
        // A failed monitor may win while the data future owns a transaction
        // or a receiving file. Drop the owned future before any final SQL or
        // observation; dropping a Pin<&mut _> would leave that future alive.
        drop(operation);
        self.save_control(&item,&control).await?;
        let confirmed=offset.load(Ordering::Acquire).max(last).min(item.spec.size.0);
        self.runtime.managed_attempt_progress(id,confirmed,item.spec.size.0).await.map_err(io_error)?;
        if outcome.is_ok() {
            if let Ok(snapshot)=connection.get_transfer(id).await {self.observe_receipt(&item,&control,snapshot).await?;}
            self.complete(item.spec.id,item.spec.size.0).await?;
            self.runtime.finish_managed_attempt(id,"completed",None).await.map_err(io_error)?;return Ok(());
        }
        let message=outcome.as_ref().err().unwrap().to_string();
        if let Some(error)=control.cleanup_error() {
            let message=format!("Could not clean cancelled receiving data: {error}");
            self.advance(item.spec.id,FileItemState::Failed,confirmed,Some(&message)).await?;
            self.runtime.finish_managed_attempt(id,"failed",Some(&message)).await.map_err(io_error)?;return Ok(());
        }
        if self.spec.direction==FileTransferDirection::Download {
            match control.local_publication() {
                Some(true) if !control.local_metadata_pending()=>{self.complete(item.spec.id,item.spec.size.0).await?;self.runtime.finish_managed_attempt(id,"completed",None).await.map_err(io_error)?;return Ok(());},
                Some(false)=>{self.advance(item.spec.id,FileItemState::Failed,confirmed,Some(&message)).await?;self.runtime.finish_managed_attempt(id,"failed",Some(&message)).await.map_err(io_error)?;return Ok(());},
                Some(true)|None=>{},
            }
        }
        if self.revoked.load(Ordering::Acquire) {
            self.advance(item.spec.id,FileItemState::Reconciling,confirmed,Some("Account session ended; observe the original attempt after signing in again")).await?;
            self.store.release_for_observation(&self.owner,item.spec.id,self.worker).await?;return Ok(());
        }
        if self.spec.direction==FileTransferDirection::Download && control.staging_discarded() {
            if *self.intent.borrow()==FileTransferIntent::Cancel {
                self.stop_item(item.spec.id,confirmed).await?;
                self.runtime.finish_managed_attempt(id,"cancelled",None).await.map_err(io_error)?;return Ok(());
            }
            self.advance(item.spec.id,FileItemState::Failed,confirmed,Some(&message)).await?;
            self.runtime.finish_managed_attempt(id,"failed",Some(&message)).await.map_err(io_error)?;return Ok(());
        }
        if !control.remote_requested() {
            if *self.intent.borrow()!=FileTransferIntent::Run {self.stop_item(item.spec.id,confirmed).await?;self.runtime.finish_managed_attempt(id,if *self.intent.borrow()==FileTransferIntent::Cancel{"cancelled"}else{"paused"},None).await.map_err(io_error)?;}
            else {self.advance(item.spec.id,FileItemState::Failed,confirmed,Some(&message)).await?;self.runtime.finish_managed_attempt(id,"failed",Some(&message)).await.map_err(io_error)?;}
            return Ok(());
        }
        let snapshot=connection.get_transfer(id).await;
        if let Ok(snapshot)=snapshot {
            self.observe_receipt(&item,&control,snapshot.clone()).await?;
            if self.spec.direction==FileTransferDirection::Upload && snapshot.state=="completed" && snapshot.published==Some(true) {
                self.complete(item.spec.id,item.spec.size.0).await?;self.runtime.finish_managed_attempt(id,"completed",None).await.map_err(io_error)?;return Ok(());
            }
            let stopped=matches!(snapshot.state.as_str(),"paused"|"cancelled") && (self.spec.direction==FileTransferDirection::Upload || control.durable_checkpoint().is_some())
                || self.spec.direction==FileTransferDirection::Download && snapshot.state=="completed" && control.durable_checkpoint().is_some() && !control.committing() && *self.intent.borrow()!=FileTransferIntent::Run;
            if stopped {
                let bytes=if self.spec.direction==FileTransferDirection::Upload {snapshot.offset}else{control.durable_checkpoint().unwrap()};
                self.stop_item(item.spec.id,bytes).await?;self.runtime.finish_managed_attempt(id,if *self.intent.borrow()==FileTransferIntent::Cancel{"cancelled"}else{"paused"},None).await.map_err(io_error)?;return Ok(());
            }
            if matches!(snapshot.state.as_str(),"failed"|"interrupted") && (self.spec.direction==FileTransferDirection::Download || snapshot.published==Some(false)) {
                // The stream error can be deliberately generic. The validated
                // original task receipt retains the actual receiver failure.
                let reason=snapshot.message.as_deref().filter(|message|!message.is_empty()).unwrap_or(&message);
                self.advance(item.spec.id,FileItemState::Failed,confirmed,Some(reason)).await?;self.runtime.finish_managed_attempt(id,"failed",Some(reason)).await.map_err(io_error)?;return Ok(());
            }
        }
        self.advance(item.spec.id,FileItemState::Reconciling,confirmed,Some(&message)).await?;
        self.store.release_for_observation(&self.owner,item.spec.id,self.worker).await?;
        self.runtime.finish_managed_attempt(id,"unconfirmed",Some(&message)).await.map_err(io_error)?;Ok(())
    }
    async fn save_control(&self,item:&FileBatchItem,control:&TransferControl)->Result<(),FileManagerError> {
        let evidence=control.evidence();
        if evidence.local_staging.is_some() || evidence.digest.is_some() || evidence.receipt.is_some() || evidence.local_checkpoint.is_some() {self.store.observe_attempt(&self.owner,item.spec.id,self.worker,item.attempt.ok_or(FileManagerError::Conflict)?,&evidence).await?;}
        Ok(())
    }
    async fn observe_receipt(&self,item:&FileBatchItem,control:&TransferControl,receipt:TransferSnapshot)->Result<(),FileManagerError> {
        if self.spec.direction==FileTransferDirection::Upload && receipt.state=="completed" {
            receipt.metadata_result.as_ref().ok_or(FileManagerError::Conflict)?
                .validate_for(&FileTransferMetadata {modified_at_unix_ms:item.spec.modified_at_unix_ms,unix_mode:item.spec.unix_mode})
                .map_err(|_|FileManagerError::Conflict)?;
        }
        let identity=FileExecutionIdentity::from_context(&self.expected).map_err(FileManagerError::Invalid)?;
        let remote_path=if self.spec.direction==FileTransferDirection::Upload {self.store.item_target(&self.owner,item.spec.id,self.destination_style).await?.path}else{item.spec.source.clone()};
        if receipt.request_id!=item.attempt.ok_or(FileManagerError::Conflict)? || receipt.path!=remote_path.display_label() || receipt.execution_context.as_ref().is_none_or(|context|!identity.matches_context(context)) || receipt.size>item.spec.size.0 || receipt.direction!=if self.spec.direction==FileTransferDirection::Upload{"receive"}else{"send"} || receipt.state=="completed" && (receipt.size!=item.spec.size.0 || receipt.sha256.is_none()) || receipt.sha256.as_ref().zip(control.digest().as_ref()).is_some_and(|(a,b)|a!=b) {return Err(FileManagerError::Conflict);}
        control.observe_snapshot(receipt.clone()).map_err(io_error)?;
        let evidence=AttemptEvidence {digest:receipt.sha256.clone().or_else(||control.digest()),context:receipt.execution_context.clone(),receipt:Some(receipt),local_checkpoint:control.durable_checkpoint().map(FileByteCount),..Default::default()};
        self.store.observe_attempt(&self.owner,item.spec.id,self.worker,item.attempt.unwrap(),&evidence).await
    }
    async fn observe_item(&self,item:FileBatchItem)->Result<(),FileManagerError> {
        let id=item.attempt.ok_or(FileManagerError::Conflict)?;
        if item.spec.kind==ManagedFileKind::Directory {
            if let Some(finalization)=self.store.directory_finalization(&self.owner,id).await? {
                let result=match finalization.result {Some(result)=>result,None=>self.destination.observe(id,finalization.operation).await?};
                return self.finish_directory(&item,result).await;
            }
            if let Some(ready)=self.store.directory_ready(&self.owner,item.spec.id).await? {
                // The worker stopped before persisting a finalization request.
                // No attribute write was admitted; return to the child barrier.
                return self.store.record_directory_ready(&self.owner,item.spec.id,self.worker,&ready).await;
            }
            if let Some(preparation)=self.store.directory_preparation(&self.owner,id).await? {
                let result=match preparation.result {Some(result)=>result,None=>self.destination.observe(id,preparation.operation).await?};
                return self.finish_preparation(&item,result).await;
            }
            // No durable admission means this attempt never issued a directory
            // mutation. Report a known failure; an explicit retry gets a new ID.
            self.advance(item.spec.id,FileItemState::Failed,0,Some("Directory preparation stopped before submission")).await?;
            return Ok(());
        }
        let connection=self.runtime.file_connection(self.spec.device,self.spec.force_relay).await.map_err(io_error)?;
        let snapshot=connection.get_transfer(id).await.map_err(io_error)?;
        let control=TransferControl::default();self.observe_receipt(&item,&control,snapshot.clone()).await?;
        if self.spec.direction==FileTransferDirection::Upload && snapshot.state=="completed" && snapshot.published==Some(true) {
            self.complete(item.spec.id,item.spec.size.0).await?;self.runtime.managed_attempt_progress(id,item.spec.size.0,item.spec.size.0).await.map_err(io_error)?;self.runtime.finish_managed_attempt(id,"completed",None).await.map_err(io_error)?;return Ok(());
        }
        // Download publication belongs to the receiving process; a source-side
        // "completed" receipt alone can never declare the local file complete.
        if matches!(snapshot.state.as_str(),"running"|"stopping") {
            let intent=if *self.intent.borrow()==FileTransferIntent::Cancel {FileTransferIntent::Cancel}else{FileTransferIntent::Pause};
            let _=connection.stop_transfer(id,intent).await;
        }
        if matches!(snapshot.state.as_str(),"paused"|"cancelled"|"failed"|"interrupted") && self.spec.direction==FileTransferDirection::Upload && snapshot.published==Some(false) {
            self.stop_item(item.spec.id,snapshot.offset).await?;return Ok(());
        }
        if self.spec.direction==FileTransferDirection::Download && matches!(snapshot.state.as_str(),"paused"|"cancelled"|"failed"|"interrupted"|"completed") {
            let evidence=self.store.attempt_evidence(&self.owner,id).await?;
            if evidence.local_staging_discarded {
                if *self.intent.borrow()==FileTransferIntent::Cancel {
                    self.stop_item(item.spec.id,item.confirmed.0).await?;
                    self.runtime.finish_managed_attempt(id,"cancelled",None).await.map_err(io_error)?;return Ok(());
                }
                let message="The receiving staging file was discarded; retry starts a new receiving object";
                self.advance(item.spec.id,FileItemState::Failed,item.confirmed.0,Some(message)).await?;
                self.runtime.finish_managed_attempt(id,"failed",Some(message)).await.map_err(io_error)?;return Ok(());
            }
            let target=self.store.item_target(&self.owner,item.spec.id,self.destination_style).await?;
            let destination=pab_file_manager::decode_path(&target.path)?;
            let parent=self.store.item_destination_parent(&self.owner,item.spec.id,&target.path,self.destination_style).await?;
            if let Some(checkpoint)=crate::connection::restore_download_checkpoint_owned(id,&destination,item.spec.size.0,evidence.local_checkpoint.map(|value|value.0),Some(&parent),evidence.local_staging.as_ref()).await.map_err(io_error)? {
                if snapshot.state=="failed" || evidence.digest.is_none() {
                    let message=snapshot.message.as_deref().unwrap_or("The source did not confirm a resumable file");
                    self.advance(item.spec.id,FileItemState::Failed,item.confirmed.0,Some(message)).await?;self.runtime.finish_managed_attempt(id,"failed",Some(message)).await.map_err(io_error)?;
                } else if snapshot.state=="cancelled" && *self.intent.borrow()==FileTransferIntent::Run {
                    self.advance(item.spec.id,FileItemState::Cancelled,item.confirmed.0,None).await?;self.runtime.finish_managed_attempt(id,"cancelled",None).await.map_err(io_error)?;
                } else {
                    self.store.observe_attempt(&self.owner,item.spec.id,self.worker,id,&AttemptEvidence {local_checkpoint:Some(FileByteCount(checkpoint)),..Default::default()}).await?;
                    let intent=*self.intent.borrow();self.stop_item(item.spec.id,checkpoint).await?;self.runtime.finish_managed_attempt(id,if intent==FileTransferIntent::Cancel{"cancelled"}else{"paused"},None).await.map_err(io_error)?;
                }
                return Ok(());
            }
            if evidence.local_metadata_requested.is_some() && evidence.local_metadata_result.is_none() {
                if evidence.local_publication_intent && evidence.local_checkpoint==Some(item.spec.size) {
                    if let Some(digest)=evidence.digest.as_deref() {
                        if let Some(result)=crate::connection::recover_download_metadata(&destination,item.spec.size.0,digest,evidence.local_metadata_requested.as_ref().unwrap(),Some(&parent)).await.map_err(io_error)? {
                            self.store.observe_attempt(&self.owner,item.spec.id,self.worker,id,&AttemptEvidence {local_published:true,local_metadata_result:Some(result),..Default::default()}).await?;
                            self.complete(item.spec.id,item.spec.size.0).await?;self.runtime.managed_attempt_progress(id,item.spec.size.0,item.spec.size.0).await.map_err(io_error)?;self.runtime.finish_managed_attempt(id,"completed",None).await.map_err(io_error)?;return Ok(());
                        }
                    }
                }
                self.store.release_for_observation(&self.owner,item.spec.id,self.worker).await?;return Ok(());
            }
            if evidence.local_published {
                self.complete(item.spec.id,item.spec.size.0).await?;self.runtime.managed_attempt_progress(id,item.spec.size.0,item.spec.size.0).await.map_err(io_error)?;self.runtime.finish_managed_attempt(id,"completed",None).await.map_err(io_error)?;return Ok(());
            }
            if evidence.local_publication_intent && evidence.local_checkpoint==Some(item.spec.size) {
                if let Some(digest)=evidence.digest.as_deref() {
                    if crate::connection::verify_download_publication(&destination,item.spec.size.0,digest,Some(&parent)).await.map_err(io_error)? {
                        self.complete(item.spec.id,item.spec.size.0).await?;self.runtime.managed_attempt_progress(id,item.spec.size.0,item.spec.size.0).await.map_err(io_error)?;self.runtime.finish_managed_attempt(id,"completed",None).await.map_err(io_error)?;return Ok(());
                    }
                }
            }
        }
        self.store.release_for_observation(&self.owner,item.spec.id,self.worker).await?;Ok(())
    }
}
