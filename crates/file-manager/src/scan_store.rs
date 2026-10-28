use crate::{BatchStore,FileManagerError,now_ms};
use pab_protocol::{FileBatchItemSpec,FileTransferIntent,ManagedFileKind,NativeFilePath,RequestId};
use sqlx::Row;

pub struct PlannedItem {pub spec:FileBatchItemSpec,pub parent:Option<RequestId>,pub skip:Option<String>}
pub struct ScanDirectory {pub item:RequestId,pub source:NativeFilePath,pub destination:NativeFilePath,pub relative_name:String}
impl BatchStore {
    pub(crate) async fn init_scan(&self)->Result<(),FileManagerError> {
        for statement in [
            "CREATE TABLE IF NOT EXISTS fm_dependencies(child TEXT PRIMARY KEY REFERENCES fm_items(id) ON DELETE CASCADE,parent TEXT NOT NULL REFERENCES fm_items(id) ON DELETE CASCADE)",
            "CREATE INDEX IF NOT EXISTS fm_dependency_parent ON fm_dependencies(parent)",
            "CREATE TABLE IF NOT EXISTS fm_scan_directories(item TEXT PRIMARY KEY REFERENCES fm_items(id) ON DELETE CASCADE,batch TEXT NOT NULL REFERENCES fm_batches(id) ON DELETE CASCADE,done INTEGER NOT NULL DEFAULT 0)",
            "CREATE INDEX IF NOT EXISTS fm_scan_pending ON fm_scan_directories(batch,done)",
        ] {sqlx::query(statement).execute(&self.pool).await?;}
        Ok(())
    }
    pub async fn scan_intent(&self,owner:&str,batch:RequestId)->Result<FileTransferIntent,FileManagerError> {
        let value:Option<String> = sqlx::query_scalar("SELECT intent FROM fm_batches WHERE id=? AND owner=?").bind(batch.to_string()).bind(owner).fetch_optional(&self.pool).await?;
        serde_json::from_str(&format!("\"{}\"",value.ok_or(FileManagerError::NotFound)?)).map_err(Into::into)
    }
    /// Metadata, dependencies and subdirectory queue entries commit together.
    /// A resumed directory page either verifies its existing immutable item or
    /// inserts it once; it can never increase totals twice.
    pub async fn append_planned_batch(&self,owner:&str,batch:RequestId,items:&[PlannedItem])->Result<(),FileManagerError> {
        if items.len()>256 {return Err(FileManagerError::Invalid("plan at most 256 items per page"));}
        let mut tx = self.write_transaction().await?;
        let row = sqlx::query("SELECT total_size FROM fm_batches WHERE id=? AND owner=? AND scan_complete=0 AND intent='run' AND NOT EXISTS(SELECT 1 FROM fm_batch_preflight p WHERE p.batch=fm_batches.id)").bind(batch.to_string()).bind(owner).fetch_optional(&mut *tx).await?.ok_or(FileManagerError::NotFound)?;
        let mut size = row.try_get::<String,_>("total_size")?.parse::<u64>().map_err(|_|FileManagerError::Invalid("invalid batch size"))?;
        let mut seq:i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fm_items WHERE batch=?").bind(batch.to_string()).fetch_one(&mut *tx).await?;
        for item in items {
            item.spec.validate().map_err(FileManagerError::Invalid)?;
            let data = serde_json::to_string(&item.spec)?;
            let previous = sqlx::query("SELECT batch,spec FROM fm_items WHERE id=?").bind(item.spec.id.to_string()).fetch_optional(&mut *tx).await?;
            if let Some(previous) = previous {
                if previous.try_get::<String,_>("batch")? != batch.to_string() || previous.try_get::<String,_>("spec")? != data {return Err(FileManagerError::Conflict);}
                continue;
            }
            if seq >= 1_000_000 {return Err(FileManagerError::Capacity);}
            size = size.checked_add(item.spec.size.0).ok_or(FileManagerError::Capacity)?;
            sqlx::query("INSERT INTO fm_items(id,batch,seq,spec,state,message,updated) VALUES(?,?,?,?,?,?,?)").bind(item.spec.id.to_string()).bind(batch.to_string()).bind(seq).bind(data).bind(if item.skip.is_some(){"skipped"}else{"queued"}).bind(item.skip.as_deref()).bind(now_ms()).execute(&mut *tx).await?;
            if let Some(parent) = item.parent {
                let valid:i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fm_items WHERE id=? AND batch=?").bind(parent.to_string()).bind(batch.to_string()).fetch_one(&mut *tx).await?;
                if valid != 1 {return Err(FileManagerError::Invalid("invalid directory dependency"));}
                sqlx::query("INSERT INTO fm_dependencies(child,parent) VALUES(?,?)").bind(item.spec.id.to_string()).bind(parent.to_string()).execute(&mut *tx).await?;
            }
            if item.spec.kind == ManagedFileKind::Directory && item.skip.is_none() {
                sqlx::query("INSERT INTO fm_scan_directories(item,batch) VALUES(?,?)").bind(item.spec.id.to_string()).bind(batch.to_string()).execute(&mut *tx).await?;
            }
            seq+=1;
        }
        sqlx::query("UPDATE fm_batches SET total_size=?,revision=revision+1,updated=? WHERE id=?").bind(size.to_string()).bind(now_ms()).bind(batch.to_string()).execute(&mut *tx).await?;
        tx.commit().await?;Ok(())
    }
    pub async fn next_scan_directory(&self,owner:&str,batch:RequestId)->Result<Option<ScanDirectory>,FileManagerError> {
        let row = sqlx::query("SELECT i.spec FROM fm_scan_directories d JOIN fm_items i ON i.id=d.item JOIN fm_batches b ON b.id=d.batch WHERE d.batch=? AND b.owner=? AND b.intent='run' AND d.done=0 ORDER BY i.seq LIMIT 1").bind(batch.to_string()).bind(owner).fetch_optional(&self.pool).await?;
        row.map(|row|->Result<_,FileManagerError>{let spec:FileBatchItemSpec=serde_json::from_str(row.try_get("spec")?)?;Ok(ScanDirectory {item:spec.id,source:spec.source,destination:spec.destination,relative_name:spec.relative_name})}).transpose()
    }
    pub async fn complete_scan_directory(&self,owner:&str,batch:RequestId,item:RequestId)->Result<(),FileManagerError> {
        let changed=sqlx::query("UPDATE fm_scan_directories SET done=1 WHERE item=? AND batch=? AND EXISTS(SELECT 1 FROM fm_batches WHERE id=? AND owner=? AND intent='run')").bind(item.to_string()).bind(batch.to_string()).bind(batch.to_string()).bind(owner).execute(&self.pool).await?.rows_affected();
        if changed != 1 {return Err(FileManagerError::Conflict);}Ok(())
    }
    pub async fn list(&self,owner:&str,limit:u16)->Result<Vec<crate::BatchSnapshot>,FileManagerError> {
        self.list_with_dismissed(owner,limit,true).await
    }
    pub async fn list_visible(&self,owner:&str,limit:u16)->Result<Vec<crate::BatchSnapshot>,FileManagerError> {
        self.list_with_dismissed(owner,limit,false).await
    }
    pub async fn clear_completed(&self,owner:&str)->Result<Vec<String>,FileManagerError> {
        // Keep execution receipts and transferred files. Dismiss only successful,
        // settled batches belonging to this account, atomically with the state check.
        let mut tx=self.write_transaction().await?;
        let ids=sqlx::query_scalar("INSERT INTO fm_batch_dismissals(batch) SELECT b.id FROM fm_batches b WHERE b.owner=? AND b.scan_complete=1 AND b.message IS NULL AND NOT EXISTS(SELECT 1 FROM fm_items i WHERE i.batch=b.id AND (i.worker IS NOT NULL OR i.state NOT IN ('completed','skipped'))) AND NOT EXISTS(SELECT 1 FROM fm_batch_dismissals d WHERE d.batch=b.id) RETURNING batch").bind(owner).fetch_all(&mut *tx).await?;
        tx.commit().await?;Ok(ids)
    }
    async fn list_with_dismissed(&self,owner:&str,limit:u16,include_dismissed:bool)->Result<Vec<crate::BatchSnapshot>,FileManagerError> {
        if limit == 0 || limit>100 {return Err(FileManagerError::Invalid("invalid batch list limit"));}
        let ids:Vec<String> = sqlx::query_scalar("SELECT b.id FROM fm_batches b WHERE b.owner=? AND (? OR NOT EXISTS(SELECT 1 FROM fm_batch_dismissals d WHERE d.batch=b.id)) ORDER BY CASE WHEN b.message IS NULL AND ((b.scan_complete=0 AND (b.intent!='cancel' OR b.scan_stopped=0)) OR EXISTS(SELECT 1 FROM fm_items i WHERE i.batch=b.id AND i.state NOT IN ('completed','skipped','failed','cancelled'))) THEN 0 ELSE 1 END,b.created DESC,b.id DESC LIMIT ?").bind(owner).bind(include_dismissed).bind(i64::from(limit)).fetch_all(&self.pool).await?;
        let mut result=Vec::with_capacity(ids.len());
        for id in ids {result.push(self.snapshot(owner,id.parse().map_err(|_|FileManagerError::Invalid("invalid batch ID"))?).await?);}
        Ok(result)
    }
}
