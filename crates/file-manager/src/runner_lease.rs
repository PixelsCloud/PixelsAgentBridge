use crate::{BatchStore,FileManagerError,now_ms};
use pab_protocol::RequestId;
use sqlx::Row;
impl BatchStore {
    /// After the owning coordinator has stopped polling its workers, relinquish
    /// only that worker's leases. Open or committing attempts remain uncertain;
    /// account revocation is never a receiver pause acknowledgement.
    pub async fn retire_worker(&self,owner:&str,batch:RequestId,worker:RequestId)->Result<(),FileManagerError> {
        self.retire_worker_with_reason(owner,batch,worker,None).await
    }
    /// Persist coordinator failures on its own unfinished rows, without
    /// inventing a receiver result or changing another coordinator's work.
    pub async fn retire_worker_with_reason(&self,owner:&str,batch:RequestId,worker:RequestId,reason:Option<&str>)->Result<(),FileManagerError> {
        let mut tx=self.write_transaction().await?;
        let owned:i64=sqlx::query_scalar("SELECT COUNT(*) FROM fm_batches b JOIN fm_workers w ON w.owner=b.owner WHERE b.id=? AND b.owner=? AND w.id=?").bind(batch.to_string()).bind(owner).bind(worker.to_string()).fetch_one(&mut *tx).await?;
        if owned!=1{return Err(FileManagerError::NotFound);}
        sqlx::query("UPDATE fm_attempts SET state='reconciling' WHERE id IN (SELECT attempt FROM fm_items WHERE batch=? AND worker=?)").bind(batch.to_string()).bind(worker.to_string()).execute(&mut *tx).await?;
        sqlx::query("UPDATE fm_items SET state='reconciling',worker=NULL,message=COALESCE(?,message),revision=revision+1,updated=? WHERE batch=? AND worker=?").bind(reason.map(|text|text.chars().take(2048).collect::<String>())).bind(now_ms()).bind(batch.to_string()).bind(worker.to_string()).execute(&mut *tx).await?;
        if let Some(reason)=reason {
            // A draining receiver may already have released its item lease.
            // The still-owned batch runner lets us retain the fatal cause on
            // those uncertain rows, without touching another live runner.
            sqlx::query("UPDATE fm_items SET message=?,revision=revision+1,updated=? WHERE batch=? AND worker IS NULL AND state IN ('reconciling','unconfirmed','interrupted') AND EXISTS(SELECT 1 FROM fm_runners r WHERE r.batch=fm_items.batch AND r.worker=?)")
                .bind(reason.chars().take(2048).collect::<String>()).bind(now_ms()).bind(batch.to_string()).bind(worker.to_string()).execute(&mut *tx).await?;
        }
        sqlx::query("DELETE FROM fm_runners WHERE batch=? AND worker=?").bind(batch.to_string()).bind(worker.to_string()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM fm_workers WHERE id=? AND owner=? AND NOT EXISTS(SELECT 1 FROM fm_items WHERE worker=?) AND NOT EXISTS(SELECT 1 FROM fm_runners WHERE worker=?)").bind(worker.to_string()).bind(owner).bind(worker.to_string()).bind(worker.to_string()).execute(&mut *tx).await?;
        sqlx::query("UPDATE fm_batches SET revision=revision+1,updated=? WHERE id=?").bind(now_ms()).bind(batch.to_string()).execute(&mut *tx).await?;
        tx.commit().await?;Ok(())
    }
    pub(crate) async fn init_runners(&self)->Result<(),FileManagerError> {
        sqlx::query("CREATE TABLE IF NOT EXISTS fm_runners(batch TEXT PRIMARY KEY REFERENCES fm_batches(id) ON DELETE CASCADE,worker TEXT NOT NULL REFERENCES fm_workers(id))").execute(&self.pool).await?;Ok(())
    }
    pub async fn claim_runner(&self,owner:&str,batch:RequestId,worker:RequestId)->Result<(),FileManagerError> {
        let mut tx=self.write_transaction().await?;
        let owned:i64=sqlx::query_scalar("SELECT COUNT(*) FROM fm_batches b,fm_workers w WHERE b.id=? AND b.owner=? AND w.id=? AND w.owner=b.owner AND w.heartbeat>=? AND NOT EXISTS(SELECT 1 FROM fm_batch_preflight p WHERE p.batch=b.id)")
            .bind(batch.to_string()).bind(owner).bind(worker.to_string()).bind(now_ms()-30_000).fetch_one(&mut *tx).await?;
        if owned!=1 {return Err(FileManagerError::NotFound);}
        if let Some(row)=sqlx::query("SELECT r.worker,w.heartbeat FROM fm_runners r LEFT JOIN fm_workers w ON w.id=r.worker WHERE r.batch=?").bind(batch.to_string()).fetch_optional(&mut *tx).await? {
            if row.try_get::<String,_>("worker")?!=worker.to_string() && row.try_get::<Option<i64>,_>("heartbeat")?.is_some_and(|value|value>=now_ms()-30_000) {return Err(FileManagerError::Conflict);}
        }
        sqlx::query("INSERT INTO fm_runners(batch,worker) VALUES(?,?) ON CONFLICT(batch) DO UPDATE SET worker=excluded.worker").bind(batch.to_string()).bind(worker.to_string()).execute(&mut *tx).await?;
        tx.commit().await?;Ok(())
    }
    pub async fn verify_runner_control(&self,owner:&str,batch:RequestId,worker:RequestId)->Result<(),FileManagerError> {
        let row=sqlx::query("SELECT r.worker,w.heartbeat FROM fm_batches b LEFT JOIN fm_runners r ON r.batch=b.id LEFT JOIN fm_workers w ON w.id=r.worker WHERE b.id=? AND b.owner=?").bind(batch.to_string()).bind(owner).fetch_optional(&self.pool).await?.ok_or(FileManagerError::NotFound)?;
        if row.try_get::<Option<String>,_>("worker")?.is_some_and(|existing|existing!=worker.to_string()) && row.try_get::<Option<i64>,_>("heartbeat")?.is_some_and(|value|value>=now_ms()-30_000) {return Err(FileManagerError::Conflict);}
        Ok(())
    }
    pub async fn release_runner(&self,owner:&str,batch:RequestId,worker:RequestId)->Result<(),FileManagerError> {
        let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM fm_items i JOIN fm_batches b ON b.id=i.batch WHERE i.batch=? AND b.owner=? AND i.worker=?")
            .bind(batch.to_string()).bind(owner).bind(worker.to_string()).fetch_one(&self.pool).await?;
        if active!=0 {return Err(FileManagerError::Conflict);}
        sqlx::query("DELETE FROM fm_runners WHERE batch=? AND worker=? AND EXISTS(SELECT 1 FROM fm_batches b WHERE b.id=fm_runners.batch AND b.owner=?)").bind(batch.to_string()).bind(worker.to_string()).bind(owner).execute(&self.pool).await?;Ok(())
    }
}
