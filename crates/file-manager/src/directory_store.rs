//! Directory creation/readiness and final attributes are different durable facts.
use crate::{BatchStore,FileManagerError};
use pab_protocol::{FileManagerOperation,FileManagerResult,FileTransferMetadata,NativeFilePath,RequestId};
use serde::{Serialize,Deserialize};
use sqlx::Row;

#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize)]
pub struct DirectoryReady {pub path:NativeFilePath,pub identity:String}
#[derive(Debug,Clone)]
pub struct DirectoryFinalization {pub request_id:RequestId,pub operation:FileManagerOperation,pub result:Option<FileManagerResult>}

impl BatchStore {
    /// A coordinator ACK is justified by the original persisted operation and
    /// result, not by the item's current phase or a new observation of its path.
    pub async fn recorded_directory_receipt(&self,owner:&str,batch:RequestId,attempt:RequestId)->Result<Option<crate::FileMutationReceipt>,FileManagerError> {
        let row=sqlx::query("SELECT p.operation AS preparation,p.result AS preparation_result,f.operation AS finalization,f.result AS finalization_result FROM fm_batches b LEFT JOIN fm_attempts a ON a.id=? AND EXISTS(SELECT 1 FROM fm_items i WHERE i.id=a.item AND i.batch=b.id) LEFT JOIN fm_directory_preparations p ON p.attempt=a.id LEFT JOIN fm_directory_finalizations f ON f.attempt=a.id WHERE b.id=? AND b.owner=?")
            .bind(attempt.to_string()).bind(batch.to_string()).bind(owner).fetch_optional(&self.pool).await?.ok_or(FileManagerError::NotFound)?;
        let preparation=row.try_get::<Option<&str>,_>("preparation_result")?;
        let finalization=row.try_get::<Option<&str>,_>("finalization_result")?;
        let (operation,result)=match (preparation,finalization) {
            (Some(result),None)=>(row.try_get::<&str,_>("preparation")?,result),
            (None,Some(result))=>(row.try_get::<&str,_>("finalization")?,result),
            (None,None)=>return Ok(None),
            _=>return Err(FileManagerError::Conflict),
        };
        let receipt=crate::FileMutationReceipt {request_id:attempt,operation:serde_json::from_str(operation)?,result:serde_json::from_str(result)?};
        if !matches!(receipt.operation,FileManagerOperation::PrepareDirectory {..}|FileManagerOperation::SetDirectoryMetadata {..}) {return Err(FileManagerError::Conflict);}
        receipt.validate()?;Ok(Some(receipt))
    }
    pub(crate) async fn init_directories(&self)->Result<(),FileManagerError> {
        for statement in [
            "CREATE TABLE IF NOT EXISTS fm_batch_destinations(batch TEXT PRIMARY KEY REFERENCES fm_batches(id) ON DELETE CASCADE,data TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS fm_directory_ready(item TEXT PRIMARY KEY REFERENCES fm_items(id) ON DELETE CASCADE,data TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS fm_directory_finalizations(attempt TEXT PRIMARY KEY REFERENCES fm_attempts(id) ON DELETE CASCADE,operation TEXT NOT NULL,result TEXT)",
            "CREATE TABLE IF NOT EXISTS fm_directory_preparations(attempt TEXT PRIMARY KEY REFERENCES fm_attempts(id) ON DELETE CASCADE,operation TEXT NOT NULL,result TEXT)",
        ] {sqlx::query(statement).execute(&self.pool).await?;}
        Ok(())
    }
    /// An item's parent is either the admission-time root or its ready parent
    /// item. Neither is re-observed/adopted after a restart or conflict choice.
    pub async fn item_destination_parent(&self,owner:&str,item:RequestId,target:&NativeFilePath,style:pab_protocol::PathStyle)->Result<pab_protocol::FileTransferParentConstraint,FileManagerError> {
        let row=sqlx::query("SELECT d.parent,r.data AS parent_ready,b.data AS root_ready FROM fm_items i JOIN fm_batches batch ON batch.id=i.batch LEFT JOIN fm_dependencies d ON d.child=i.id LEFT JOIN fm_directory_ready r ON r.item=d.parent LEFT JOIN fm_batch_destinations b ON b.batch=i.batch WHERE i.id=? AND batch.owner=?")
            .bind(item.to_string()).bind(owner).fetch_optional(&self.pool).await?.ok_or(FileManagerError::NotFound)?;
        let data=row.try_get::<Option<String>,_>(if row.try_get::<Option<String>,_>("parent")?.is_some(){"parent_ready"}else{"root_ready"})?.ok_or(FileManagerError::Invalid("destination parent identity was not recorded"))?;
        let parent:DirectoryReady=serde_json::from_str(&data)?;
        // Reconstruct using the frozen parent so a selected trailing separator
        // remains valid without weakening the actual receiver object check.
        if crate::native_join(&parent.path,&crate::native_name(target,style)?,style)?!=*target {return Err(FileManagerError::Conflict);}
        let constraint=pab_protocol::FileTransferParentConstraint {identity:parent.identity};constraint.validate().map_err(FileManagerError::Invalid)?;Ok(constraint)
    }
    pub async fn directory_ready(&self,owner:&str,item:RequestId)->Result<Option<DirectoryReady>,FileManagerError> {
        let row=sqlx::query("SELECT d.data FROM fm_items i JOIN fm_batches b ON b.id=i.batch LEFT JOIN fm_directory_ready d ON d.item=i.id WHERE i.id=? AND b.owner=?")
            .bind(item.to_string()).bind(owner).fetch_optional(&self.pool).await?.ok_or(FileManagerError::NotFound)?;
        row.try_get::<Option<String>,_>("data")?.map(|value|serde_json::from_str(&value)).transpose().map_err(Into::into)
    }
    /// Release the creation worker without claiming that final attributes are
    /// finished. Pause/cancel arriving during creation wins the next stage.
    pub async fn record_directory_ready(&self,owner:&str,item:RequestId,worker:RequestId,ready:&DirectoryReady)->Result<(),FileManagerError> {
        let data=serde_json::to_string(ready)?;
        let mut tx=self.write_transaction().await?;
        let row=sqlx::query("SELECT i.batch,i.spec,i.attempt,b.intent,d.data,t.data AS target FROM fm_items i JOIN fm_batches b ON b.id=i.batch LEFT JOIN fm_directory_ready d ON d.item=i.id LEFT JOIN fm_item_targets t ON t.item=i.id WHERE i.id=? AND b.owner=? AND i.worker=? AND i.state IN ('preparing','publishing','pause_requested','cancel_requested','reconciling','unconfirmed')")
            .bind(item.to_string()).bind(owner).bind(worker.to_string()).fetch_optional(&mut *tx).await?.ok_or(FileManagerError::Conflict)?;
        let spec:pab_protocol::FileBatchItemSpec=serde_json::from_str(row.try_get("spec")?)?;
        if spec.kind!=pab_protocol::ManagedFileKind::Directory {return Err(FileManagerError::Conflict);}
        let target:crate::ItemTarget=serde_json::from_str(row.try_get::<Option<&str>,_>("target")?.ok_or(FileManagerError::Conflict)?)?;
        if target.path!=ready.path || ready.identity.is_empty() || ready.identity.len()>192 {return Err(FileManagerError::Conflict);}
        if row.try_get::<Option<&str>,_>("data")?.is_some_and(|old|old!=data) {return Err(FileManagerError::Conflict);}
        let attempt:Option<&str>=row.try_get("attempt")?;
        if let Some(attempt)=attempt {
            let finalizing:i64=sqlx::query_scalar("SELECT COUNT(*) FROM fm_directory_finalizations WHERE attempt=?").bind(attempt).fetch_one(&mut *tx).await?;
            if finalizing!=0 {return Err(FileManagerError::Conflict);}
        }
        sqlx::query("INSERT INTO fm_directory_ready(item,data) VALUES(?,?) ON CONFLICT(item) DO NOTHING").bind(item.to_string()).bind(data).execute(&mut *tx).await?;
        let next=match row.try_get::<&str,_>("intent")? {"pause"=>"paused","cancel"=>"cancelled",_=>"waiting_children"};
        sqlx::query("UPDATE fm_items SET state=?,worker=NULL,message=NULL,revision=revision+1,updated=? WHERE id=?").bind(next).bind(crate::now_ms()).bind(item.to_string()).execute(&mut *tx).await?;
        if let Some(attempt)=attempt {sqlx::query("UPDATE fm_attempts SET state=?,ended=? WHERE id=?").bind(next).bind(crate::now_ms()).bind(attempt).execute(&mut *tx).await?;}
        bump(&mut tx,row.try_get("batch")?).await?;tx.commit().await?;Ok(())
    }
    /// Persist the original metadata request and irrevocable stage together.
    /// Returns None when a concurrent stop has won before that gate.
    pub async fn begin_directory_finalization(&self,owner:&str,item:RequestId,worker:RequestId)->Result<Option<DirectoryFinalization>,FileManagerError> {
        let mut tx=self.write_transaction().await?;
        let row=sqlx::query("SELECT i.batch,i.spec,i.attempt,i.state,b.intent,d.data FROM fm_items i JOIN fm_batches b ON b.id=i.batch JOIN fm_directory_ready d ON d.item=i.id WHERE i.id=? AND b.owner=? AND i.worker=?")
            .bind(item.to_string()).bind(owner).bind(worker.to_string()).fetch_optional(&mut *tx).await?.ok_or(FileManagerError::NotFound)?;
        if row.try_get::<&str,_>("intent")?!="run" || row.try_get::<&str,_>("state")?!="preparing" {return Ok(None);}
        let unfinished:i64=sqlx::query_scalar("SELECT COUNT(*) FROM fm_dependencies d JOIN fm_items c ON c.id=d.child WHERE d.parent=? AND c.state NOT IN ('completed','skipped')").bind(item.to_string()).fetch_one(&mut *tx).await?;
        if unfinished!=0 {return Err(FileManagerError::Conflict);}
        let ready:DirectoryReady=serde_json::from_str(row.try_get("data")?)?;
        let spec:pab_protocol::FileBatchItemSpec=serde_json::from_str(row.try_get("spec")?)?;
        if spec.kind!=pab_protocol::ManagedFileKind::Directory {return Err(FileManagerError::Conflict);}
        let request_id:RequestId=row.try_get::<&str,_>("attempt")?.parse().map_err(|_|FileManagerError::Conflict)?;
        let operation=FileManagerOperation::SetDirectoryMetadata {path:ready.path,expected_object:ready.identity,metadata:FileTransferMetadata {modified_at_unix_ms:spec.modified_at_unix_ms,unix_mode:spec.unix_mode}};
        sqlx::query("INSERT INTO fm_directory_finalizations(attempt,operation) VALUES(?,?)").bind(request_id.to_string()).bind(serde_json::to_string(&operation)?).execute(&mut *tx).await?;
        sqlx::query("UPDATE fm_items SET state='publishing',revision=revision+1,updated=? WHERE id=?").bind(crate::now_ms()).bind(item.to_string()).execute(&mut *tx).await?;
        sqlx::query("UPDATE fm_attempts SET state='publishing' WHERE id=?").bind(request_id.to_string()).execute(&mut *tx).await?;
        bump(&mut tx,row.try_get("batch")?).await?;tx.commit().await?;
        Ok(Some(DirectoryFinalization {request_id,operation,result:None}))
    }
    pub async fn directory_finalization(&self,owner:&str,attempt:RequestId)->Result<Option<DirectoryFinalization>,FileManagerError> {
        let row=sqlx::query("SELECT d.operation,d.result FROM fm_attempts a JOIN fm_items i ON i.id=a.item JOIN fm_batches b ON b.id=i.batch LEFT JOIN fm_directory_finalizations d ON d.attempt=a.id WHERE a.id=? AND b.owner=?")
            .bind(attempt.to_string()).bind(owner).fetch_optional(&self.pool).await?.ok_or(FileManagerError::NotFound)?;
        let Some(operation)=row.try_get::<Option<&str>,_>("operation")? else {return Ok(None);};
        Ok(Some(DirectoryFinalization {request_id:attempt,operation:serde_json::from_str(operation)?,result:row.try_get::<Option<&str>,_>("result")?.map(serde_json::from_str).transpose()?}))
    }
    pub async fn record_directory_outcome(&self,owner:&str,item:RequestId,worker:RequestId,attempt:RequestId,result:&FileManagerResult)->Result<(),FileManagerError> {
        let mut tx=self.write_transaction().await?;
        let row=sqlx::query("SELECT i.batch,d.operation,d.result FROM fm_items i JOIN fm_batches b ON b.id=i.batch JOIN fm_directory_finalizations d ON d.attempt=i.attempt WHERE i.id=? AND b.owner=? AND i.worker=? AND i.attempt=? AND i.state IN ('publishing','reconciling','unconfirmed')")
            .bind(item.to_string()).bind(owner).bind(worker.to_string()).bind(attempt.to_string()).fetch_optional(&mut *tx).await?.ok_or(FileManagerError::Conflict)?;
        let operation:FileManagerOperation=serde_json::from_str(row.try_get("operation")?)?;
        let request=pab_protocol::FileManagerRequest {schema_version:1,request_id:attempt,execution:pab_protocol::ExecutionSelection::Service {},expected_execution:None,operation};
        let reply=pab_protocol::FileManagerReply {schema_version:1,request_id:attempt,execution_context:None,result:result.clone()};
        reply.validate_for(&request).map_err(|_|FileManagerError::Conflict)?;
        if !matches!(result,FileManagerResult::DirectoryMetadata {..}|FileManagerResult::Error {..}) {return Err(FileManagerError::Conflict);}
        if matches!(result,FileManagerResult::Error {error} if matches!(error.code.as_str(),"unconfirmed"|"outcome_unconfirmed")) {return Err(FileManagerError::Interrupted);}
        let data=serde_json::to_string(result)?;
        if row.try_get::<Option<&str>,_>("result")?.is_some_and(|old|old!=data) {return Err(FileManagerError::Conflict);}
        sqlx::query("UPDATE fm_directory_finalizations SET result=? WHERE attempt=?").bind(data).bind(attempt.to_string()).execute(&mut *tx).await?;
        bump(&mut tx,row.try_get("batch")?).await?;tx.commit().await?;Ok(())
    }
}
async fn bump(tx:&mut sqlx::Transaction<'_,sqlx::Sqlite>,batch:&str)->Result<(),sqlx::Error> {
    sqlx::query("UPDATE fm_batches SET revision=revision+1,updated=? WHERE id=?").bind(crate::now_ms()).bind(batch).execute(&mut **tx).await?;Ok(())
}
