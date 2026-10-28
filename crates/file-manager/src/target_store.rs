use crate::{BatchStore,FileManagerError,native_join,native_name};
use pab_protocol::{FileBatchItemSpec,FileConflictPolicy,NativeFilePath,PathStyle,RequestId};
use serde::{Deserialize,Serialize};
use sqlx::Row;

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct ItemTarget {pub path:NativeFilePath,pub overwrite:bool,pub choice:FileConflictPolicy}
impl BatchStore {
    /// An explicit retry requeues only known failures. Completed/skipped files
    /// and uncertain attempts retain their original facts and request IDs.
    pub async fn retry_failed(&self,owner:&str,batch:RequestId)->Result<u64,FileManagerError> {
        let mut tx=self.write_transaction().await?;
        let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM fm_batches WHERE id=? AND owner=? AND scan_complete=1 AND message IS NULL AND intent!='cancel'").bind(batch.to_string()).bind(owner).fetch_one(&mut *tx).await?;
        if valid!=1 {return Err(FileManagerError::NotFound);}
        let unsafe_items:i64=sqlx::query_scalar("SELECT COUNT(*) FROM fm_items WHERE batch=? AND (worker IS NOT NULL OR state NOT IN ('completed','skipped','failed','cancelled'))").bind(batch.to_string()).fetch_one(&mut *tx).await?;
        if unsafe_items!=0 {return Err(FileManagerError::Conflict);}
        let changed=sqlx::query("UPDATE fm_items SET state=CASE WHEN EXISTS(SELECT 1 FROM fm_directory_ready r WHERE r.item=fm_items.id) THEN 'waiting_children' ELSE 'queued' END,message=NULL,revision=revision+1,updated=? WHERE batch=? AND state='failed' AND worker IS NULL").bind(crate::now_ms()).bind(batch.to_string()).execute(&mut *tx).await?.rows_affected();
        if changed>0 {sqlx::query("UPDATE fm_batches SET intent='run',revision=revision+1,updated=? WHERE id=?").bind(crate::now_ms()).bind(batch.to_string()).execute(&mut *tx).await?;}
        tx.commit().await?;Ok(changed)
    }
    pub async fn propagate_blocked_children(&self,owner:&str,batch:RequestId)->Result<u64,FileManagerError> {
        let mut tx=self.write_transaction().await?;
        let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM fm_batches WHERE id=? AND owner=?").bind(batch.to_string()).bind(owner).fetch_one(&mut *tx).await?;
        if valid!=1 {return Err(FileManagerError::NotFound);}
        let mut changed=sqlx::query("WITH RECURSIVE blocked(id,state) AS (SELECT d.child,p.state FROM fm_dependencies d JOIN fm_items p ON p.id=d.parent WHERE p.batch=? AND p.state IN ('failed','skipped','cancelled') AND NOT EXISTS(SELECT 1 FROM fm_directory_ready r WHERE r.item=p.id) UNION ALL SELECT d.child,b.state FROM fm_dependencies d JOIN blocked b ON b.id=d.parent) UPDATE fm_items SET state=(SELECT state FROM blocked b WHERE b.id=fm_items.id LIMIT 1),message='Parent directory was not transferred',revision=revision+1,updated=? WHERE batch=? AND state='queued' AND worker IS NULL AND id IN (SELECT id FROM blocked)")
            .bind(batch.to_string()).bind(crate::now_ms()).bind(batch.to_string()).execute(&mut *tx).await?.rows_affected();
        // A failed subtree must remain writable for an explicit retry. Drain
        // other children first, then fail the directory without finalizing it.
        loop {
            let parents=sqlx::query("UPDATE fm_items SET state='failed',message='A child did not complete; directory attributes were deferred',revision=revision+1,updated=? WHERE batch=? AND state='waiting_children' AND worker IS NULL AND EXISTS(SELECT 1 FROM fm_dependencies d JOIN fm_items c ON c.id=d.child WHERE d.parent=fm_items.id AND c.state IN ('failed','cancelled')) AND NOT EXISTS(SELECT 1 FROM fm_dependencies d JOIN fm_items c ON c.id=d.child WHERE d.parent=fm_items.id AND c.state NOT IN ('completed','skipped','failed','cancelled'))")
                .bind(crate::now_ms()).bind(batch.to_string()).execute(&mut *tx).await?.rows_affected();
            changed+=parents;if parents==0 {break;}
        }
        if changed>0 {sqlx::query("UPDATE fm_batches SET revision=revision+1,updated=? WHERE id=?").bind(crate::now_ms()).bind(batch.to_string()).execute(&mut *tx).await?;}
        tx.commit().await?;Ok(changed)
    }
    pub(crate) async fn init_targets(&self)->Result<(),FileManagerError> {
        sqlx::query("CREATE TABLE IF NOT EXISTS fm_batch_dismissals(batch TEXT PRIMARY KEY REFERENCES fm_batches(id) ON DELETE CASCADE)").execute(&self.pool).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS fm_batch_conflict_choices(batch TEXT PRIMARY KEY REFERENCES fm_batches(id) ON DELETE CASCADE,choice TEXT NOT NULL)").execute(&self.pool).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS fm_item_targets(item TEXT PRIMARY KEY REFERENCES fm_items(id) ON DELETE CASCADE,data TEXT NOT NULL)").execute(&self.pool).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS fm_conflict_choices(item TEXT PRIMARY KEY REFERENCES fm_items(id) ON DELETE CASCADE,choice TEXT NOT NULL)").execute(&self.pool).await?;
        Ok(())
    }
    pub async fn item_target(&self,owner:&str,item:RequestId,style:PathStyle)->Result<ItemTarget,FileManagerError> {
        let row=sqlx::query("SELECT i.spec,b.spec AS batch_spec,t.data,p.data AS parent_target,COALESCE(c.choice,bc.choice) AS choice FROM fm_items i JOIN fm_batches b ON b.id=i.batch LEFT JOIN fm_item_targets t ON t.item=i.id LEFT JOIN fm_dependencies d ON d.child=i.id LEFT JOIN fm_item_targets p ON p.item=d.parent LEFT JOIN fm_conflict_choices c ON c.item=i.id LEFT JOIN fm_batch_conflict_choices bc ON bc.batch=b.id WHERE i.id=? AND b.owner=?")
            .bind(item.to_string()).bind(owner).fetch_optional(&self.pool).await?.ok_or(FileManagerError::NotFound)?;
        let mut target:ItemTarget=if let Some(data)=row.try_get::<Option<String>,_>("data")? {serde_json::from_str(&data)?}else {
            let spec:FileBatchItemSpec=serde_json::from_str(row.try_get("spec")?)?;
            let batch:pab_protocol::FileBatchSpec=serde_json::from_str(row.try_get("batch_spec")?)?;
            let path=if let Some(data)=row.try_get::<Option<String>,_>("parent_target")? {let parent:ItemTarget=serde_json::from_str(&data)?;native_join(&parent.path,&native_name(&spec.destination,style)?,style)?}else{spec.destination};
            ItemTarget {path,overwrite:false,choice:batch.conflict}
        };
        if let Some(choice)=row.try_get::<Option<String>,_>("choice")? {target.choice=serde_json::from_str(&choice)?;}
        Ok(target)
    }
    pub async fn save_item_target(&self,owner:&str,item:RequestId,worker:RequestId,target:&ItemTarget)->Result<(),FileManagerError> {
        target.path.validate().map_err(FileManagerError::Invalid)?;
        let mut tx=self.write_transaction().await?;
        let row=sqlx::query("SELECT t.data FROM fm_items i JOIN fm_batches b ON b.id=i.batch LEFT JOIN fm_item_targets t ON t.item=i.id WHERE i.id=? AND b.owner=? AND i.worker=? AND i.state IN ('preparing','pause_requested','cancel_requested')")
            .bind(item.to_string()).bind(owner).bind(worker.to_string()).fetch_optional(&mut *tx).await?.ok_or(FileManagerError::NotFound)?;
        if let Some(data)=row.try_get::<Option<String>,_>("data")? {
            let previous:ItemTarget=serde_json::from_str(&data)?;
            if previous.path!=target.path || previous.overwrite!=target.overwrite {
                let accepted:i64=sqlx::query_scalar("SELECT COUNT(*) FROM fm_attempts a WHERE a.item=? AND (EXISTS(SELECT 1 FROM fm_attempt_evidence e WHERE e.attempt=a.id) OR EXISTS(SELECT 1 FROM fm_directory_preparations d WHERE d.attempt=a.id AND COALESCE(json_extract(d.result,'$.result'),'unknown')!='error'))")
                    .bind(item.to_string()).fetch_one(&mut *tx).await?;
                if accepted!=0 {return Err(FileManagerError::Conflict);}
            }
        }
        sqlx::query("INSERT INTO fm_item_targets(item,data) VALUES(?,?) ON CONFLICT(item) DO UPDATE SET data=excluded.data")
            .bind(item.to_string()).bind(serde_json::to_string(target)?).execute(&mut *tx).await?;
        tx.commit().await?;Ok(())
    }
    pub async fn resolve_conflict(&self,owner:&str,batch:RequestId,item:RequestId,choice:FileConflictPolicy)->Result<(),FileManagerError> {self.resolve_conflict_scope(owner,batch,item,choice,false).await}
    pub async fn resolve_conflict_scope(&self,owner:&str,batch:RequestId,item:RequestId,choice:FileConflictPolicy,apply_remaining:bool)->Result<(),FileManagerError> {
        if choice==FileConflictPolicy::Ask {return Err(FileManagerError::Invalid("choose replace, skip or keep both"));}
        let mut tx=self.write_transaction().await?;
        let batch:Option<String>=sqlx::query_scalar("SELECT i.batch FROM fm_items i JOIN fm_batches b ON b.id=i.batch WHERE i.id=? AND i.batch=? AND b.owner=? AND i.worker IS NULL AND i.state='waiting_conflict'")
            .bind(item.to_string()).bind(batch.to_string()).bind(owner).fetch_optional(&mut *tx).await?;
        let batch=batch.ok_or(FileManagerError::NotFound)?;
        sqlx::query("INSERT INTO fm_conflict_choices(item,choice) VALUES(?,?) ON CONFLICT(item) DO UPDATE SET choice=excluded.choice").bind(item.to_string()).bind(serde_json::to_string(&choice)?).execute(&mut *tx).await?;
        if apply_remaining {
            sqlx::query("INSERT INTO fm_batch_conflict_choices(batch,choice) VALUES(?,?) ON CONFLICT(batch) DO UPDATE SET choice=excluded.choice").bind(&batch).bind(serde_json::to_string(&choice)?).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO fm_conflict_choices(item,choice) SELECT id,? FROM fm_items WHERE batch=? AND state='waiting_conflict' AND worker IS NULL ON CONFLICT(item) DO UPDATE SET choice=excluded.choice").bind(serde_json::to_string(&choice)?).bind(&batch).execute(&mut *tx).await?;
            sqlx::query("UPDATE fm_items SET state='queued',revision=revision+1,message=NULL WHERE batch=? AND state='waiting_conflict' AND worker IS NULL").bind(&batch).execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE fm_items SET state='queued',revision=revision+1,message=NULL WHERE id=?").bind(item.to_string()).execute(&mut *tx).await?;
        sqlx::query("UPDATE fm_batches SET revision=revision+1,updated=? WHERE id=?").bind(crate::now_ms()).bind(batch).execute(&mut *tx).await?;
        tx.commit().await?;Ok(())
    }
}
