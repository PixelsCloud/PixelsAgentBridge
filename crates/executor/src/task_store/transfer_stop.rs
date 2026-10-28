//! Stop intent is not a completed stop. Only the transfer's bound recorder may
//! confirm a flushed checkpoint, and publication already in progress wins.
use super::{TaskStore,TaskStoreError};
use pab_protocol::{FileTransferIntent,OperatorRef,RequestId,TransferSnapshot};
use sqlx::Row;
impl TaskStore {
    pub async fn request_transfer_stop(&self,actor:OperatorRef,id:RequestId,intent:FileTransferIntent)->Result<TransferSnapshot,TaskStoreError> {
        if intent==FileTransferIntent::Run {return Err(TaskStoreError::RequestConflict);}
        let original=self.get_transfer(actor,id).await?;
        if original.finished_at_unix_ms.is_some() || original.state=="committing" {return Ok(original);}
        if original.execution_context.is_none() {return Err(TaskStoreError::RequestConflict);}
        let value=if intent==FileTransferIntent::Cancel{"cancel"}else{"pause"};
        let mut tx=self.write_transaction().await?;
        sqlx::query("INSERT INTO transfer_stop_requests(request_id,intent,requested) SELECT request_id,?,? FROM transfer_operations WHERE request_id=? AND state IN ('running','stopping') ON CONFLICT(request_id) DO UPDATE SET intent=CASE WHEN transfer_stop_requests.intent='cancel' THEN 'cancel' ELSE excluded.intent END")
            .bind(value).bind(super::operation::now_unix_ms()).bind(id.to_string()).execute(&mut *tx).await?;
        tx.commit().await?;
        self.get_transfer(actor,id).await
    }
    pub(crate) async fn transfer_stop_pending(&self,id:RequestId)->Result<bool,TaskStoreError> {
        Ok(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM transfer_stop_requests s JOIN transfer_operations o ON o.request_id=s.request_id WHERE s.request_id=? AND o.state='running'").bind(id.to_string()).fetch_one(&self.pool).await?!=0)
    }
    pub(crate) async fn transfer_cancel_pending(&self,id:RequestId)->Result<bool,TaskStoreError> {
        Ok(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM transfer_stop_requests s JOIN transfer_operations o ON o.request_id=s.request_id WHERE s.request_id=? AND s.intent='cancel' AND o.state IN ('running','stopping')")
            .bind(id.to_string()).fetch_one(&self.pool).await?!=0)
    }
    pub(crate) async fn confirm_transfer_stopped(&self,id:RequestId,offset:u64,size:u64)->Result<(),TaskStoreError> {
        if offset>size || size>i64::MAX as u64 {return Err(TaskStoreError::OffsetTooLarge);}
        let mut tx=self.write_transaction().await?;
        let row=sqlx::query("SELECT s.intent,o.size,o.offset,o.state FROM transfer_stop_requests s JOIN transfer_operations o ON o.request_id=s.request_id WHERE s.request_id=?")
            .bind(id.to_string()).fetch_optional(&mut *tx).await?.ok_or(TaskStoreError::RequestConflict)?;
        if row.try_get::<String,_>("state")?!="running" || row.try_get::<i64,_>("size")?!=size as i64 || row.try_get::<i64,_>("offset")?>offset as i64 {return Err(TaskStoreError::RequestConflict);}
        if !matches!(row.try_get::<&str,_>("intent")?,"pause"|"cancel") {return Err(TaskStoreError::RequestConflict);}
        sqlx::query("UPDATE transfer_operations SET state=?,offset=?,finished_at_unix_ms=?,message=NULL WHERE request_id=? AND state='running'")
            .bind("stopping").bind(offset as i64).bind(None::<i64>).bind(id.to_string()).execute(&mut *tx).await?;
        tx.commit().await?;Ok(())
    }
    pub(crate) async fn finalize_transfer_stopped(&self,id:RequestId)->Result<(),TaskStoreError> {
        let changed=sqlx::query("UPDATE transfer_operations SET state=CASE (SELECT intent FROM transfer_stop_requests s WHERE s.request_id=transfer_operations.request_id) WHEN 'pause' THEN 'paused' WHEN 'cancel' THEN 'cancelled' ELSE state END,finished_at_unix_ms=? WHERE request_id=? AND state='stopping' AND EXISTS(SELECT 1 FROM transfer_stop_requests s WHERE s.request_id=transfer_operations.request_id)")
            .bind(super::operation::now_unix_ms()).bind(id.to_string()).execute(&self.pool).await?.rows_affected();
        if changed!=1 {return Err(TaskStoreError::RequestConflict);}Ok(())
    }
}
