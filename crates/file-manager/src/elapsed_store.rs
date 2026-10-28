//! Bounded, idempotent elapsed-time checkpoints belonging to one live runner.
//! Persist observed monotonic durations only; never charge process downtime.
use crate::{BatchStore, FileManagerError, now_ms};
use pab_protocol::{FileByteCount, RequestId};
use sqlx::Row;

impl BatchStore {
    pub(crate) async fn init_elapsed(&self) -> Result<(), FileManagerError> {
        sqlx::query("CREATE TABLE IF NOT EXISTS fm_elapsed(batch TEXT PRIMARY KEY REFERENCES fm_batches(id) ON DELETE CASCADE,total_ms TEXT NOT NULL DEFAULT '0',worker TEXT NOT NULL,token TEXT NOT NULL,checkpoint_ms TEXT NOT NULL DEFAULT '0')")
            .execute(&self.pool).await?;
        Ok(())
    }

    /// Start a new monotonic segment after claiming the runner. The aggregate
    /// survives release/restart; a fresh token rejects late old checkpoints,
    /// including those from an earlier round of the same worker.
    pub async fn begin_elapsed(&self, owner: &str, batch: RequestId, worker: RequestId) -> Result<RequestId, FileManagerError> {
        let mut tx = self.write_transaction().await?;
        let owned: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fm_batches b JOIN fm_runners r ON r.batch=b.id JOIN fm_workers w ON w.id=r.worker AND w.owner=b.owner WHERE b.id=? AND b.owner=? AND r.worker=? AND w.heartbeat>=?")
            .bind(batch.to_string()).bind(owner).bind(worker.to_string()).bind(now_ms()-30_000).fetch_one(&mut *tx).await?;
        if owned != 1 { return Err(FileManagerError::NotFound); }
        let token = RequestId::new();
        sqlx::query("INSERT INTO fm_elapsed(batch,worker,token) VALUES(?,?,?) ON CONFLICT(batch) DO UPDATE SET worker=excluded.worker,token=excluded.token,checkpoint_ms='0'")
            .bind(batch.to_string()).bind(worker.to_string()).bind(token.to_string()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(token)
    }

    pub async fn checkpoint_elapsed(&self, owner: &str, batch: RequestId, worker: RequestId, token: RequestId, cumulative: FileByteCount) -> Result<(), FileManagerError> {
        let mut tx = self.write_transaction().await?;
        let row = sqlx::query("SELECT e.total_ms,e.checkpoint_ms FROM fm_elapsed e JOIN fm_batches b ON b.id=e.batch JOIN fm_runners r ON r.batch=b.id AND r.worker=e.worker JOIN fm_workers w ON w.id=r.worker AND w.owner=b.owner WHERE b.id=? AND b.owner=? AND e.worker=? AND e.token=? AND w.heartbeat>=?")
            .bind(batch.to_string()).bind(owner).bind(worker.to_string()).bind(token.to_string()).bind(now_ms()-30_000)
            .fetch_optional(&mut *tx).await?.ok_or(FileManagerError::NotFound)?;
        let parse = |field| -> Result<u64, FileManagerError> {
            row.try_get::<String,_>(field)?.parse().map_err(|_| FileManagerError::Invalid("invalid elapsed checkpoint"))
        };
        let previous = parse("checkpoint_ms")?;
        if cumulative.0 > previous {
            let total = parse("total_ms")?.checked_add(cumulative.0-previous).ok_or(FileManagerError::Capacity)?;
            sqlx::query("UPDATE fm_elapsed SET total_ms=?,checkpoint_ms=? WHERE batch=?")
                .bind(total.to_string()).bind(cumulative.0.to_string()).bind(batch.to_string()).execute(&mut *tx).await?;
            sqlx::query("UPDATE fm_batches SET revision=revision+1 WHERE id=?").bind(batch.to_string()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
