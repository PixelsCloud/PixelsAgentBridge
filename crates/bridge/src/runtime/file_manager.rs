use super::{BridgeRuntime,RuntimeError};
use pab_file_manager::{FileBrowser,FileManagerError};
use pab_protocol::{DeviceRef,ExecutionSelection,FileManagerOperation,FileManagerReply,FileManagerRequest,FileManagerResult,NativeFilePath,RequestId};
use std::{future::Future,pin::Pin,sync::Arc,time::Duration};
use tokio::sync::watch;

impl BridgeRuntime {
    /// Observe only this file profile. Status polling never opens a transport
    /// or borrows the desktop/terminal connection's path.
    pub async fn file_profile_observation(&self,device:DeviceRef,force_relay:bool)->Option<(pab_transport::ConnectionPath,Option<u64>)> {
        let session=self.inner.file_devices.lock().await.get(&(device,force_relay)).cloned()?;
        let connection=session.cached_connection().await?;let transport=connection.connection();
        transport.selected_path().map(|path|(path,transport.selected_path_rtt().map(|rtt|rtt.as_millis().min(u128::from(u64::MAX)) as u64)))
    }
}

#[derive(Clone)]
pub struct RemoteFileBrowser {runtime:Arc<BridgeRuntime>,device:DeviceRef,execution:ExecutionSelection,force_relay:bool,owner:Option<String>,expected:Option<pab_protocol::ExecutionContext>}
impl RemoteFileBrowser {
    pub fn new(runtime:Arc<BridgeRuntime>,device:DeviceRef,execution:ExecutionSelection,force_relay:bool)->Self {Self {runtime,device,execution,force_relay,owner:None,expected:None}}
    pub fn owned(runtime:Arc<BridgeRuntime>,device:DeviceRef,execution:ExecutionSelection,force_relay:bool,owner:String)->Self {Self {runtime,device,execution,force_relay,owner:Some(owner),expected:None}}
    pub fn with_expected_context(mut self,expected:pab_protocol::ExecutionContext)->Self {self.expected=Some(expected);self}
    fn check_context(&self,reply:&FileManagerReply)->Result<(),FileManagerError> {
        if self.expected.as_ref().is_some_and(|expected|match pab_protocol::FileExecutionIdentity::from_context(expected) {
            Ok(identity)=>reply.execution_context.as_ref().is_none_or(|actual|!identity.matches_context(actual)),
            Err(_)=>reply.execution_context.as_ref()!=Some(expected),
        }){return Err(FileManagerError::Conflict);}Ok(())
    }
    /// Original-ID lookup only. Missing/expired facts never resubmit a mutation.
    pub async fn observe_request(&self,id:RequestId,operation:FileManagerOperation)->Result<Option<FileManagerReply>,FileManagerError> {
        if let Some(owner)=&self.owner && self.runtime.file_operation_owner().await.map_err(|error|failure(error,id))?!=*owner{return Err(FileManagerError::NotFound);}
        let response=self.runtime.get_filesystem_profile(self.device,id,Some(self.force_relay)).await.map_err(|error|failure(error,id))?;
        if matches!(response.state.as_str(),"running"|"cancel_requested"|"unconfirmed"){return Ok(None);}
        let reply=match response.file_manager {
            Some(reply)=>reply,
            None if matches!(response.state.as_str(),"failed"|"cancelled") && response.error.is_some()=>FileManagerReply {schema_version:pab_protocol::FILE_MANAGER_SCHEMA_VERSION,request_id:id,execution_context:response.execution_context,result:FileManagerResult::Error {error:response.error.unwrap()}},
            None=>return Ok(None),
        };
        let request=FileManagerRequest {expected_execution:self.expected.as_ref().map(pab_protocol::FileExecutionIdentity::from_context).transpose().map_err(FileManagerError::Invalid)?,schema_version:pab_protocol::FILE_MANAGER_SCHEMA_VERSION,request_id:id,execution:self.execution,operation};
        reply.validate_for(&request).map_err(FileManagerError::Invalid)?;self.check_context(&reply)?;Ok(Some(reply))
    }
    pub async fn execute_request(&self,id:RequestId,operation:FileManagerOperation,cancel:watch::Receiver<bool>)->Result<FileManagerReply,FileManagerError> {
        if let Some(owner)=&self.owner && self.runtime.file_operation_owner().await.map_err(|error|failure(error,id))?!=*owner {return Err(FileManagerError::NotFound);}
        let expected=self.expected.as_ref().map(pab_protocol::FileExecutionIdentity::from_context).transpose().map_err(FileManagerError::Invalid)?;
        let execution=if let Some(expected)=&expected {
            self.runtime.file_connection(self.device,self.force_relay).await.map_err(|error|failure(error,id))?.resolve_file_execution(expected).await.map_err(|error|failure(error,id))?
        }else{self.execution};
        let request=FileManagerRequest {expected_execution:expected,schema_version:pab_protocol::FILE_MANAGER_SCHEMA_VERSION,request_id:id,execution,operation};
        let mut response=match self.runtime.file_manager_profile(self.device,&request,self.force_relay).await {
            Ok(result)=>result.reply,
            Err(original)=>self.runtime.get_filesystem_profile(self.device,id,Some(self.force_relay)).await.map_err(|_|failure(original,id))?,
        };
        let deadline=tokio::time::Instant::now()+Duration::from_secs(120);
        let mut cancel_sent=false;
        while matches!(response.state.as_str(),"running"|"cancel_requested"|"unconfirmed") {
            if *cancel.borrow() && response.supports_cancel() && !cancel_sent {
                response=self.runtime.cancel_filesystem_profile(self.device,id,Some(self.force_relay)).await.map_err(|error|failure(error,id))?;
                cancel_sent=true;
                if !matches!(response.state.as_str(),"running"|"cancel_requested"|"unconfirmed"){break;}
            }
            if tokio::time::Instant::now()>=deadline {return Err(failure("original directory request remains unconfirmed",id));}
            tokio::time::sleep(Duration::from_millis(250)).await;
            response=self.runtime.get_filesystem_profile(self.device,id,Some(self.force_relay)).await.map_err(|error|failure(error,id))?;
        }
        let reply=response.file_manager.unwrap_or_else(||FileManagerReply {schema_version:pab_protocol::FILE_MANAGER_SCHEMA_VERSION,request_id:id,execution_context:response.execution_context,result:FileManagerResult::Error {error:response.error.unwrap_or_else(||pab_protocol::FileSystemError {code:response.state,phase:"remote_files".into(),message:"Remote request returned no file-manager result".into()})}});
        reply.validate_for(&request).map_err(FileManagerError::Invalid)?;
        self.check_context(&reply)?;
        Ok(reply)
    }
}
fn failure(error:impl std::fmt::Display,id:RequestId)->FileManagerError {FileManagerError::Io(std::io::Error::other(format!("{error} · {id}")))}
impl FileBrowser for RemoteFileBrowser {
    fn execute<'a>(&'a self,operation:FileManagerOperation,cancel:watch::Receiver<bool>)->Pin<Box<dyn Future<Output=Result<FileManagerReply,FileManagerError>>+Send+'a>> {Box::pin(self.execute_request(RequestId::new(),operation,cancel))}
}

impl BridgeRuntime {
    /// Canonical account scope, verified against control-server synchronization.
    /// A local guest endpoint may be reused across several signed-in users.
    pub async fn file_operation_owner(&self)->Result<String,RuntimeError> {
        self.inner.wait_account_ready().await?;
        let local=self.inner.account_store.read().map_err(|e|RuntimeError::TaskOperation(e.to_string()))?;
        let user=local.user.ok_or_else(||RuntimeError::TaskOperation("sign in before file transfer".into()))?;
        let origin=self.inner.store.account_origin.as_ref().ok_or_else(||RuntimeError::TaskOperation("account server origin is unavailable".into()))?;
        Ok(format!("{origin}\n{}",user.id))
    }
    pub async fn begin_managed_attempt(&self,owner:&str,id:RequestId,device:DeviceRef,direction:&str,source:&NativeFilePath,destination:&NativeFilePath,overwrite:bool)->Result<(),RuntimeError> {
        if self.file_operation_owner().await?!=owner {return Err(RuntimeError::TaskOperation("file transfer account changed".into()));}
        let local=self.inner.account_store.read().map_err(|e|RuntimeError::TaskOperation(e.to_string()))?;
        let expected=local.user.as_ref().and_then(|user|self.inner.store.account_origin.as_ref().map(|origin|format!("{origin}\n{}",user.id)));
        if expected.as_deref()!=Some(owner) {return Err(RuntimeError::TaskOperation("file transfer account changed".into()));}
        // Register the batch coordinator before creating the shared history
        // record. Legacy recovery must never adopt this attempt, open a shared
        // desktop profile for it, or count an older resumed sender twice.
        let mut tx=pab_file_manager::begin_write_transaction(&self.inner.store.pool,"UPDATE runtime_managed_file_attempts SET scope=scope WHERE 0").await.map_err(super::RuntimeStoreError::from)?;
        let unmanaged:i64=sqlx::query_scalar("SELECT COUNT(*) FROM runtime_operations o WHERE o.id=? AND NOT EXISTS(SELECT 1 FROM runtime_managed_file_attempts m WHERE m.id=o.id)").bind(id.to_string()).fetch_one(&mut *tx).await.map_err(super::RuntimeStoreError::from)?;
        if unmanaged!=0{return Err(RuntimeError::TaskOperation("attempt ID belongs to another transfer coordinator".into()));}
        sqlx::query("INSERT INTO runtime_managed_file_attempts(id,scope) VALUES(?,?) ON CONFLICT(id) DO NOTHING").bind(id.to_string()).bind(owner).execute(&mut *tx).await.map_err(super::RuntimeStoreError::from)?;
        let scope:String=sqlx::query_scalar("SELECT scope FROM runtime_managed_file_attempts WHERE id=?").bind(id.to_string()).fetch_one(&mut *tx).await.map_err(super::RuntimeStoreError::from)?;
        if scope!=owner{return Err(RuntimeError::TaskOperation("attempt belongs to another account".into()));}
        tx.commit().await.map_err(super::RuntimeStoreError::from)?;
        self.prepare_transfer_record(id,device,direction,&source.display_label(),&destination.display_label(),overwrite).await?;
        if let Some(user)=local.user {
            let attribution=pab_protocol::UserAttribution {user_id:user.id,username:user.username};
            // The general insert trigger may have captured an unassigned
            // session. Fill that null attribution once, while never replacing
            // a previously frozen user on an original attempt.
            let original:String=sqlx::query_scalar("SELECT user_json FROM runtime_operation_users WHERE operation_id=?").bind(id.to_string()).fetch_one(&self.inner.store.pool).await.map_err(super::RuntimeStoreError::from)?;
            let original:Option<pab_protocol::UserAttribution>=serde_json::from_str(&original).map_err(super::RuntimeStoreError::from)?;
            if original.as_ref().is_some_and(|old|old.user_id!=attribution.user_id){return Err(RuntimeError::TaskOperation("attempt belongs to another account".into()));}
            if original.is_none() {
                sqlx::query("UPDATE runtime_operation_users SET user_json=? WHERE operation_id=? AND user_json='null'").bind(serde_json::to_string(&attribution).map_err(super::RuntimeStoreError::from)?).bind(id.to_string()).execute(&self.inner.store.pool).await.map_err(super::RuntimeStoreError::from)?;
            }
        }
        Ok(())
    }
    pub async fn managed_attempt_progress(&self,id:RequestId,offset:u64,size:u64)->Result<(),RuntimeError> {self.inner.store.operation_progress(id,offset,size).await.map_err(Into::into)}
    pub async fn finish_managed_attempt(&self,id:RequestId,state:&str,message:Option<&str>)->Result<(),RuntimeError> {
        if !matches!(state,"completed"|"paused"|"cancelled"|"failed"|"interrupted"|"unconfirmed") {return Err(RuntimeError::TaskOperation("invalid transfer outcome".into()));}
        if state=="unconfirmed" {
            sqlx::query("UPDATE runtime_operations SET state='unconfirmed',message=? WHERE id=? AND owner_session_id=? AND finished_at_unix_ms IS NULL").bind(message).bind(id.to_string()).bind(&self.inner.session_id).execute(&self.inner.store.pool).await.map_err(super::RuntimeStoreError::from)?;
        } else {self.inner.store.finish_operation(id,state,message).await?;}
        Ok(())
    }
}
