//! Attribute outcomes belong to the original accepted receive operation.
use super::{TaskStore,TaskStoreError};
use pab_protocol::{FileTransferMetadataResult,FileTransferRequest,RequestId};
use sqlx::Row;

impl TaskStore {
    pub(crate) async fn record_transfer_metadata(&self,id:RequestId,result:&FileTransferMetadataResult)->Result<(),TaskStoreError> {
        let mut tx=self.write_transaction().await?;
        let row=sqlx::query("SELECT e.request_json,o.state,o.direction,p.request_id AS publication,m.result_json FROM transfer_operations o JOIN transfer_execution e ON e.request_id=o.request_id LEFT JOIN transfer_publications p ON p.request_id=o.request_id LEFT JOIN transfer_metadata_results m ON m.request_id=o.request_id WHERE o.request_id=?")
            .bind(id.to_string()).fetch_optional(&mut *tx).await?.ok_or(TaskStoreError::NotFound)?;
        let request:FileTransferRequest=serde_json::from_str(row.try_get("request_json")?)?;
        result.validate_for(request.metadata.as_ref().ok_or(TaskStoreError::RequestConflict)?).map_err(|_|TaskStoreError::RequestConflict)?;
        if row.try_get::<&str,_>("direction")?!="receive" || row.try_get::<Option<&str>,_>("publication")?.is_none() || !matches!(row.try_get::<&str,_>("state")?,"committing"|"completed") {return Err(TaskStoreError::RequestConflict);}
        if let Some(existing)=row.try_get::<Option<&str>,_>("result_json")? {
            if serde_json::from_str::<FileTransferMetadataResult>(existing)?!=*result {return Err(TaskStoreError::RequestConflict);}
        } else {
            sqlx::query("INSERT INTO transfer_metadata_results(request_id,result_json) VALUES(?,?)").bind(id.to_string()).bind(serde_json::to_string(result)?).execute(&mut *tx).await?;
        }
        tx.commit().await?;Ok(())
    }
}
