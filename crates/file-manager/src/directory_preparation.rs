//! A directory mutation and its original receipt survive coordinator restarts.
use crate::{BatchStore, DirectoryReady, FileManagerError, ItemTarget};
use pab_protocol::{FileManagerOperation, FileManagerResult, NativeFilePath, PathStyle, RequestId};
use sqlx::Row;

#[derive(Debug, Clone)]
pub struct DirectoryPreparation {
    pub request_id: RequestId,
    pub operation: FileManagerOperation,
    pub result: Option<FileManagerResult>,
}

impl BatchStore {
    /// Commit the complete original operation before allowing the side effect.
    /// A stop that wins this transaction leaves no admitted mutation.
    pub async fn begin_directory_preparation(
        &self, owner: &str, item: RequestId, worker: RequestId,
        path: &NativeFilePath, style: PathStyle, existing_object: Option<String>,
    ) -> Result<Option<DirectoryPreparation>, FileManagerError> {
        let mut tx = self.write_transaction().await?;
        let row = sqlx::query("SELECT i.batch,i.spec,i.attempt,i.state,b.intent,t.data AS target,d.parent,r.data AS parent_ready,root.data AS root_ready FROM fm_items i JOIN fm_batches b ON b.id=i.batch JOIN fm_item_targets t ON t.item=i.id LEFT JOIN fm_dependencies d ON d.child=i.id LEFT JOIN fm_directory_ready r ON r.item=d.parent LEFT JOIN fm_batch_destinations root ON root.batch=i.batch WHERE i.id=? AND b.owner=? AND i.worker=?")
            .bind(item.to_string()).bind(owner).bind(worker.to_string()).fetch_optional(&mut *tx).await?.ok_or(FileManagerError::NotFound)?;
        if row.try_get::<&str,_>("intent")? != "run" || row.try_get::<&str,_>("state")? != "preparing" { return Ok(None); }
        let spec: pab_protocol::FileBatchItemSpec = serde_json::from_str(row.try_get("spec")?)?;
        let target: ItemTarget = serde_json::from_str(row.try_get("target")?)?;
        if spec.kind != pab_protocol::ManagedFileKind::Directory || target.path != *path { return Err(FileManagerError::Conflict); }
        let parent_column = if row.try_get::<Option<&str>,_>("parent")?.is_some() { "parent_ready" } else { "root_ready" };
        let parent: DirectoryReady = serde_json::from_str(row.try_get::<Option<&str>,_>(parent_column)?.ok_or(FileManagerError::Invalid("destination parent identity was not recorded"))?)?;
        if crate::native_join(&parent.path, &crate::native_name(path, style)?, style)? != *path { return Err(FileManagerError::Conflict); }
        let request_id: RequestId = row.try_get::<&str,_>("attempt")?.parse().map_err(|_|FileManagerError::Conflict)?;
        let operation = FileManagerOperation::PrepareDirectory { path: path.clone(), expected_parent: parent.identity, existing_object };
        let request = pab_protocol::FileManagerRequest { schema_version:1, request_id, execution:pab_protocol::ExecutionSelection::Service {}, expected_execution:None, operation:operation.clone() };
        request.validate().map_err(FileManagerError::Invalid)?;
        let has_ready: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fm_directory_ready WHERE item=?").bind(item.to_string()).fetch_one(&mut *tx).await?;
        if has_ready != 0 { return Err(FileManagerError::Conflict); }
        sqlx::query("INSERT INTO fm_directory_preparations(attempt,operation) VALUES(?,?)").bind(request_id.to_string()).bind(serde_json::to_string(&operation)?).execute(&mut *tx).await?;
        sqlx::query("UPDATE fm_items SET state='publishing',revision=revision+1,updated=? WHERE id=?").bind(crate::now_ms()).bind(item.to_string()).execute(&mut *tx).await?;
        sqlx::query("UPDATE fm_attempts SET state='publishing' WHERE id=?").bind(request_id.to_string()).execute(&mut *tx).await?;
        bump(&mut tx,row.try_get("batch")?).await?;
        tx.commit().await?;
        Ok(Some(DirectoryPreparation { request_id, operation, result:None }))
    }

    pub async fn directory_preparation(&self, owner:&str, attempt:RequestId) -> Result<Option<DirectoryPreparation>,FileManagerError> {
        let row = sqlx::query("SELECT d.operation,d.result FROM fm_attempts a JOIN fm_items i ON i.id=a.item JOIN fm_batches b ON b.id=i.batch LEFT JOIN fm_directory_preparations d ON d.attempt=a.id WHERE a.id=? AND b.owner=?")
            .bind(attempt.to_string()).bind(owner).fetch_optional(&self.pool).await?.ok_or(FileManagerError::NotFound)?;
        let Some(operation) = row.try_get::<Option<&str>,_>("operation")? else { return Ok(None); };
        Ok(Some(DirectoryPreparation {request_id:attempt,operation:serde_json::from_str(operation)?,result:row.try_get::<Option<&str>,_>("result")?.map(serde_json::from_str).transpose()?}))
    }

    /// The validated original receipt and child readiness are one transaction.
    /// Never inspect the current name to reconstruct the object of an old task.
    pub async fn record_directory_preparation(&self,owner:&str,item:RequestId,worker:RequestId,attempt:RequestId,result:&FileManagerResult)->Result<(),FileManagerError> {
        let mut tx=self.write_transaction().await?;
        let row=sqlx::query("SELECT i.batch,b.intent,d.operation,d.result,r.data AS ready,t.data AS target FROM fm_items i JOIN fm_batches b ON b.id=i.batch JOIN fm_directory_preparations d ON d.attempt=i.attempt JOIN fm_item_targets t ON t.item=i.id LEFT JOIN fm_directory_ready r ON r.item=i.id WHERE i.id=? AND b.owner=? AND i.worker=? AND i.attempt=? AND i.state IN ('publishing','reconciling','unconfirmed')")
            .bind(item.to_string()).bind(owner).bind(worker.to_string()).bind(attempt.to_string()).fetch_optional(&mut *tx).await?.ok_or(FileManagerError::Conflict)?;
        let operation:FileManagerOperation=serde_json::from_str(row.try_get("operation")?)?;
        let request=pab_protocol::FileManagerRequest {schema_version:1,request_id:attempt,execution:pab_protocol::ExecutionSelection::Service {},expected_execution:None,operation};
        let reply=pab_protocol::FileManagerReply {schema_version:1,request_id:attempt,execution_context:None,result:result.clone()};
        reply.validate_for(&request).map_err(|_|FileManagerError::Conflict)?;
        if !matches!(result,FileManagerResult::DirectoryPrepared {..}|FileManagerResult::Error {..}) {return Err(FileManagerError::Conflict);}
        if matches!(result,FileManagerResult::Error {error} if matches!(error.code.as_str(),"unconfirmed"|"outcome_unconfirmed")) {return Err(FileManagerError::Interrupted);}
        let data=serde_json::to_string(result)?;
        if row.try_get::<Option<&str>,_>("result")?.is_some_and(|old|old!=data) {return Err(FileManagerError::Conflict);}
        sqlx::query("UPDATE fm_directory_preparations SET result=? WHERE attempt=?").bind(data).bind(attempt.to_string()).execute(&mut *tx).await?;
        if let FileManagerResult::DirectoryPrepared {path,identity,..}=result {
            let target:ItemTarget=serde_json::from_str(row.try_get("target")?)?;
            if target.path!=*path {return Err(FileManagerError::Conflict);}
            let ready=serde_json::to_string(&DirectoryReady {path:path.clone(),identity:identity.clone()})?;
            if row.try_get::<Option<&str>,_>("ready")?.is_some_and(|old|old!=ready) {return Err(FileManagerError::Conflict);}
            sqlx::query("INSERT INTO fm_directory_ready(item,data) VALUES(?,?) ON CONFLICT(item) DO NOTHING").bind(item.to_string()).bind(ready).execute(&mut *tx).await?;
            let next=match row.try_get::<&str,_>("intent")? {"pause"=>"paused","cancel"=>"cancelled",_=>"waiting_children"};
            sqlx::query("UPDATE fm_items SET state=?,worker=NULL,message=NULL,revision=revision+1,updated=? WHERE id=?").bind(next).bind(crate::now_ms()).bind(item.to_string()).execute(&mut *tx).await?;
            sqlx::query("UPDATE fm_attempts SET state=?,ended=? WHERE id=?").bind(next).bind(crate::now_ms()).bind(attempt.to_string()).execute(&mut *tx).await?;
        }
        bump(&mut tx,row.try_get("batch")?).await?;
        tx.commit().await?;Ok(())
    }
}
async fn bump(tx:&mut sqlx::Transaction<'_,sqlx::Sqlite>,batch:&str)->Result<(),sqlx::Error> {
    sqlx::query("UPDATE fm_batches SET revision=revision+1,updated=? WHERE id=?").bind(crate::now_ms()).bind(batch).execute(&mut **tx).await?;Ok(())
}
