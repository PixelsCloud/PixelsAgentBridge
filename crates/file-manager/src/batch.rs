use crate::{FileManagerError, now_ms};
use pab_protocol::{
    FileBatchItem, FileBatchItemSpec, FileBatchSpec, FileByteCount, FileItemState,
    FileTransferIntent, RequestId,
};
use serde::Serialize;
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{path::Path, time::Duration};

#[derive(Clone)]
pub struct BatchStore {
    pub(crate) pool: SqlitePool,
}
#[derive(Debug, Clone, Serialize)]
pub struct BatchSnapshot {
    pub spec: FileBatchSpec,
    pub intent: FileTransferIntent,
    pub phase: String,
    pub revision: FileByteCount,
    pub total: FileByteCount,
    pub completed: FileByteCount,
    pub skipped: FileByteCount,
    pub failed: FileByteCount,
    pub cancelled: FileByteCount,
    pub size: FileByteCount,
    pub confirmed: FileByteCount,
    /// Observed monotonic working time. Waiting, pause and process downtime
    /// are never reconstructed from wall-clock timestamps.
    pub active_elapsed_ms: FileByteCount,
    pub scan_complete: bool,
    pub message: Option<String>,
}
impl BatchStore {
    pub(crate) async fn write_transaction(&self)->Result<sqlx::Transaction<'static,sqlx::Sqlite>,sqlx::Error> {
        crate::begin_write_transaction(&self.pool,"UPDATE fm_batches SET revision=revision WHERE 0").await
    }
    #[cfg(test)]
    pub(crate) async fn forget_worker_for_test(&self, id: RequestId) {
        sqlx::query("DELETE FROM fm_workers WHERE id=?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .unwrap();
    }
    pub async fn open(database: &Path) -> Result<Self, FileManagerError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(database)
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(Duration::from_secs(10)),
            )
            .await?;
        for statement in [
            "CREATE TABLE IF NOT EXISTS fm_batches(id TEXT PRIMARY KEY,owner TEXT NOT NULL,device TEXT NOT NULL,spec TEXT NOT NULL,scan_complete INTEGER NOT NULL DEFAULT 0,scan_stopped INTEGER NOT NULL DEFAULT 0,total_size TEXT NOT NULL DEFAULT '0',confirmed TEXT NOT NULL DEFAULT '0',intent TEXT NOT NULL DEFAULT 'run',revision INTEGER NOT NULL DEFAULT 1,created INTEGER NOT NULL,updated INTEGER NOT NULL,message TEXT)",
            "CREATE INDEX IF NOT EXISTS fm_batch_owner ON fm_batches(owner,created)",
            "CREATE TABLE IF NOT EXISTS fm_batch_preflight(batch TEXT PRIMARY KEY REFERENCES fm_batches(id) ON DELETE CASCADE)",
            "CREATE TABLE IF NOT EXISTS fm_items(id TEXT PRIMARY KEY,batch TEXT NOT NULL REFERENCES fm_batches(id) ON DELETE CASCADE,seq INTEGER NOT NULL,spec TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'queued',revision INTEGER NOT NULL DEFAULT 1,confirmed TEXT NOT NULL DEFAULT '0',attempt TEXT,previous TEXT,worker TEXT,updated INTEGER NOT NULL,message TEXT,UNIQUE(batch,seq))",
            "CREATE INDEX IF NOT EXISTS fm_item_queue ON fm_items(batch,state,seq)",
            "CREATE TABLE IF NOT EXISTS fm_attempts(id TEXT PRIMARY KEY,item TEXT NOT NULL REFERENCES fm_items(id) ON DELETE CASCADE,previous TEXT,state TEXT NOT NULL,confirmed TEXT NOT NULL DEFAULT '0',digest TEXT,context TEXT,started INTEGER NOT NULL,ended INTEGER)",
            "CREATE TABLE IF NOT EXISTS fm_workers(id TEXT PRIMARY KEY,owner TEXT NOT NULL,heartbeat INTEGER NOT NULL)",
        ] {
            sqlx::query(statement).execute(&pool).await?;
        }
        let store = Self {pool};
        store.init_scan().await?;
        store.init_attempt_evidence().await?;
        store.init_targets().await?;
        store.init_directories().await?;
        store.init_runners().await?;
        store.init_elapsed().await?;
        Ok(store)
    }
    pub async fn create(
        &self,
        owner: &str,
        spec: &FileBatchSpec,
    ) -> Result<bool, FileManagerError> {
        self.create_inner(owner,spec,None,false).await
    }
    /// Save an immutable UI submission before remote preflight. Only these new
    /// reservations may acquire their first destination anchor later; opening
    /// an older, unanchored batch must never adopt today's directory.
    pub async fn reserve_submission(&self,owner:&str,spec:&FileBatchSpec)->Result<bool,FileManagerError> {
        self.create_inner(owner,spec,None,true).await
    }
    /// Admit a managed batch and freeze its destination in one transaction.
    /// Retrying admission cannot bind an existing batch to a new directory.
    pub async fn create_with_destination(&self,owner:&str,spec:&FileBatchSpec,destination:&crate::DirectoryReady)->Result<bool,FileManagerError> {
        if destination.path!=spec.destination {return Err(FileManagerError::Conflict);}
        pab_protocol::FileTransferParentConstraint {identity:destination.identity.clone()}.validate().map_err(FileManagerError::Invalid)?;
        self.create_inner(owner,spec,Some(destination),false).await
    }
    async fn create_inner(&self,owner:&str,spec:&FileBatchSpec,destination:Option<&crate::DirectoryReady>,reserve:bool)->Result<bool,FileManagerError> {
        spec.validate().map_err(FileManagerError::Invalid)?;
        if owner.is_empty() || owner.len() > 1024 {
            return Err(FileManagerError::Invalid("invalid transfer owner"));
        }
        let data = serde_json::to_string(spec)?;
        let mut tx = self.write_transaction().await?;
        let old = sqlx::query("SELECT owner,spec FROM fm_batches WHERE id=?")
            .bind(spec.id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
        if let Some(old) = old {
            if old.try_get::<&str, _>("owner")? != owner {
                return Err(FileManagerError::NotFound);
            }
            if old.try_get::<&str, _>("spec")? != data {
                return Err(FileManagerError::Conflict);
            }
            if let Some(destination)=destination {
                let original:Option<String>=sqlx::query_scalar("SELECT data FROM fm_batch_destinations WHERE batch=?").bind(spec.id.to_string()).fetch_optional(&mut *tx).await?;
                if let Some(original)=original {
                    if serde_json::from_str::<crate::DirectoryReady>(&original)?!=*destination {return Err(FileManagerError::Conflict);}
                }else {
                    let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM fm_batch_preflight p JOIN fm_batches b ON b.id=p.batch WHERE b.id=? AND b.scan_complete=0 AND NOT EXISTS(SELECT 1 FROM fm_items i WHERE i.batch=b.id) AND NOT EXISTS(SELECT 1 FROM fm_runners r WHERE r.batch=b.id)").bind(spec.id.to_string()).fetch_one(&mut *tx).await?;
                    if pending!=1 {return Err(FileManagerError::Conflict);}
                    sqlx::query("INSERT INTO fm_batch_destinations(batch,data) VALUES(?,?)").bind(spec.id.to_string()).bind(serde_json::to_string(destination)?).execute(&mut *tx).await?;
                    sqlx::query("DELETE FROM fm_batch_preflight WHERE batch=?").bind(spec.id.to_string()).execute(&mut *tx).await?;
                }
            }
            tx.commit().await?;
            return Ok(false);
        }
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM fm_batches WHERE owner=? AND scan_complete=0")
                .bind(owner)
                .fetch_one(&mut *tx)
                .await?;
        if count >= 8 {
            return Err(FileManagerError::Capacity);
        }
        sqlx::query(
            "INSERT INTO fm_batches(id,owner,device,spec,created,updated) VALUES(?,?,?,?,?,?)",
        )
        .bind(spec.id.to_string())
        .bind(owner)
        .bind(serde_json::to_string(&spec.device)?)
        .bind(data)
        .bind(now_ms())
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
        if let Some(destination)=destination {
            sqlx::query("INSERT INTO fm_batch_destinations(batch,data) VALUES(?,?)").bind(spec.id.to_string()).bind(serde_json::to_string(destination)?).execute(&mut *tx).await?;
        }else if reserve {
            sqlx::query("INSERT INTO fm_batch_preflight(batch) VALUES(?)").bind(spec.id.to_string()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(true)
    }
    pub async fn append(
        &self,
        owner: &str,
        batch: RequestId,
        items: &[FileBatchItemSpec],
    ) -> Result<(), FileManagerError> {
        if items.is_empty() {
            return Ok(());
        }
        if items.len() > 256 {
            return Err(FileManagerError::Invalid(
                "append at most 256 transfer items",
            ));
        }
        for item in items {
            item.validate().map_err(FileManagerError::Invalid)?;
        }
        let mut tx = self.write_transaction().await?;
        let scan: i64 = sqlx::query_scalar(
            "SELECT scan_complete FROM fm_batches WHERE id=? AND owner=? AND intent='run' AND NOT EXISTS(SELECT 1 FROM fm_batch_preflight p WHERE p.batch=fm_batches.id)",
        )
        .bind(batch.to_string())
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(FileManagerError::NotFound)?;
        if scan != 0 {
            return Err(FileManagerError::Conflict);
        }
        let mut seq: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fm_items WHERE batch=?")
            .bind(batch.to_string())
            .fetch_one(&mut *tx)
            .await?;
        if seq + items.len() as i64 > 1_000_000 {
            return Err(FileManagerError::Capacity);
        }
        let size: String = sqlx::query_scalar("SELECT total_size FROM fm_batches WHERE id=?")
            .bind(batch.to_string())
            .fetch_one(&mut *tx)
            .await?;
        let size = items.iter().try_fold(parse_count(&size)?, |sum, item| {
            sum.checked_add(item.size.0)
                .ok_or(FileManagerError::Capacity)
        })?;
        for item in items {
            sqlx::query("INSERT INTO fm_items(id,batch,seq,spec,updated) VALUES(?,?,?,?,?)")
                .bind(item.id.to_string())
                .bind(batch.to_string())
                .bind(seq)
                .bind(serde_json::to_string(item)?)
                .bind(now_ms())
                .execute(&mut *tx)
                .await?;
            seq += 1;
        }
        sqlx::query("UPDATE fm_batches SET total_size=? WHERE id=?")
            .bind(size.to_string())
            .bind(batch.to_string())
            .execute(&mut *tx)
            .await?;
        bump(&mut tx, batch).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn finish_scan(
        &self,
        owner: &str,
        batch: RequestId,
        error: Option<&str>,
    ) -> Result<(), FileManagerError> {
        let changed=sqlx::query("UPDATE fm_batches SET scan_complete=1,message=?,revision=revision+1,updated=? WHERE id=? AND owner=? AND scan_complete=0")
            .bind(error.map(|s|s.chars().take(2048).collect::<String>())).bind(now_ms()).bind(batch.to_string()).bind(owner).execute(&self.pool).await?.rows_affected();
        if changed != 1 {
            return Err(FileManagerError::Conflict);
        }
        Ok(())
    }
    pub async fn heartbeat(&self, owner: &str, worker: RequestId) -> Result<(), FileManagerError> {
        let changed=sqlx::query("INSERT INTO fm_workers(id,owner,heartbeat) VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET heartbeat=excluded.heartbeat WHERE fm_workers.owner=excluded.owner")
            .bind(worker.to_string()).bind(owner).bind(now_ms()).execute(&self.pool).await?.rows_affected();
        if changed != 1 {
            return Err(FileManagerError::NotFound);
        }
        Ok(())
    }
    pub async fn acknowledge_scan_stopped(
        &self,
        owner: &str,
        batch: RequestId,
    ) -> Result<(), FileManagerError> {
        let changed=sqlx::query("UPDATE fm_batches SET scan_stopped=1,revision=revision+1,updated=? WHERE id=? AND owner=? AND scan_complete=0 AND intent IN ('pause','cancel')")
            .bind(now_ms()).bind(batch.to_string()).bind(owner).execute(&self.pool).await?.rows_affected();
        if changed != 1 {
            return Err(FileManagerError::Conflict);
        }
        Ok(())
    }
    pub async fn claim(
        &self,
        owner: &str,
        batch: RequestId,
        worker: RequestId,
    ) -> Result<Option<FileBatchItem>, FileManagerError> {
        let mut tx = self.write_transaction().await?;
        let live: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM fm_workers WHERE id=? AND owner=? AND heartbeat>=?",
        )
        .bind(worker.to_string())
        .bind(owner)
        .bind(now_ms() - 30_000)
        .fetch_one(&mut *tx)
        .await?;
        if live != 1 {
            return Err(FileManagerError::Conflict);
        }
        let device:Option<String>=sqlx::query_scalar("SELECT device FROM fm_batches WHERE id=? AND owner=? AND intent='run' AND scan_complete=1 AND message IS NULL")
            .bind(batch.to_string()).bind(owner).fetch_optional(&mut *tx).await?;
        let Some(device) = device else {
            return Ok(None);
        };
        let (all,device_count):(i64,i64)=sqlx::query_as("SELECT COUNT(*),COALESCE(SUM(b.device=?),0) FROM fm_items i JOIN fm_batches b ON b.id=i.batch WHERE i.worker IS NOT NULL")
            .bind(device).fetch_one(&mut *tx).await?;
        if all >= 4 || device_count >= 2 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT i.* FROM fm_items i WHERE i.batch=? AND ((i.state='queued' AND NOT EXISTS(SELECT 1 FROM fm_directory_ready r WHERE r.item=i.id)) OR (i.state='waiting_children' AND NOT EXISTS(SELECT 1 FROM fm_dependencies d JOIN fm_items c ON c.id=d.child WHERE d.parent=i.id AND c.state NOT IN ('completed','skipped')))) AND NOT EXISTS(SELECT 1 FROM fm_dependencies d JOIN fm_items p ON p.id=d.parent WHERE d.child=i.id AND p.state!='completed' AND NOT EXISTS(SELECT 1 FROM fm_directory_ready r WHERE r.item=p.id)) ORDER BY i.seq LIMIT 1",
        )
        .bind(batch.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut item = decode_item(&row)?;
        let attempt = RequestId::new();
        let previous = item.attempt;
        sqlx::query(
            "INSERT INTO fm_attempts(id,item,previous,state,started) VALUES(?,?,?,'preparing',?)",
        )
        .bind(attempt.to_string())
        .bind(item.spec.id.to_string())
        .bind(previous.map(|n| n.to_string()))
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE fm_items SET state='preparing',revision=revision+1,previous=attempt,attempt=?,worker=?,updated=? WHERE id=?")
            .bind(attempt.to_string()).bind(worker.to_string()).bind(now_ms()).bind(item.spec.id.to_string()).execute(&mut *tx).await?;
        bump(&mut tx, batch).await?;
        tx.commit().await?;
        item.state = FileItemState::Preparing;
        item.revision.0 += 1;
        item.previous_attempt = previous;
        item.attempt = Some(attempt);
        Ok(Some(item))
    }
    pub async fn set_intent(
        &self,
        owner: &str,
        batch: RequestId,
        intent: FileTransferIntent,
    ) -> Result<(), FileManagerError> {
        let mut tx = self.write_transaction().await?;
        let old: Option<String> =
            sqlx::query_scalar("SELECT intent FROM fm_batches WHERE id=? AND owner=?")
                .bind(batch.to_string())
                .bind(owner)
                .fetch_optional(&mut *tx)
                .await?;
        let old = old.ok_or(FileManagerError::NotFound)?;
        if old == "cancel" && intent != FileTransferIntent::Cancel {
            return Err(FileManagerError::Conflict);
        }
        let value = match intent {
            FileTransferIntent::Run => "run",
            FileTransferIntent::Pause => "pause",
            FileTransferIntent::Cancel => "cancel",
        };
        sqlx::query("UPDATE fm_batches SET intent=?,scan_stopped=CASE WHEN ?='run' THEN 0 ELSE scan_stopped END,revision=revision+1,updated=? WHERE id=?")
            .bind(value).bind(value).bind(now_ms()).bind(batch.to_string()).execute(&mut *tx).await?;
        match intent {
            FileTransferIntent::Pause | FileTransferIntent::Cancel => {
                let (inactive, active) = if intent == FileTransferIntent::Pause {
                    ("paused", "pause_requested")
                } else {
                    ("cancelled", "cancel_requested")
                };
                sqlx::query("UPDATE fm_items SET state=?,revision=revision+1,updated=? WHERE batch=? AND state IN ('queued','waiting_conflict','waiting_children','paused')")
                    .bind(inactive).bind(now_ms()).bind(batch.to_string()).execute(&mut *tx).await?;
                sqlx::query("UPDATE fm_items SET state=?,revision=revision+1,updated=? WHERE batch=? AND state IN ('preparing','transferring','verifying','pause_requested')")
                    .bind(active).bind(now_ms()).bind(batch.to_string()).execute(&mut *tx).await?;
            }
            FileTransferIntent::Run => {
                sqlx::query("UPDATE fm_items SET state=CASE WHEN EXISTS(SELECT 1 FROM fm_directory_ready r WHERE r.item=fm_items.id) THEN 'waiting_children' ELSE 'queued' END,revision=revision+1,updated=? WHERE batch=? AND state='paused' AND worker IS NULL")
                    .bind(now_ms()).bind(batch.to_string()).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
    /// The worker confirms real receiver facts before calling this method.
    /// Neither the window nor a user-intent command can publish a completion.
    pub async fn advance(
        &self,
        owner: &str,
        id: RequestId,
        worker: RequestId,
        expected_revision: FileByteCount,
        next: FileItemState,
        confirmed: FileByteCount,
        message: Option<&str>,
    ) -> Result<FileBatchItem, FileManagerError> {
        let mut tx = self.write_transaction().await?;
        let row=sqlx::query("SELECT i.* FROM fm_items i JOIN fm_batches b ON b.id=i.batch WHERE i.id=? AND b.owner=? AND i.worker=?")
            .bind(id.to_string()).bind(owner).bind(worker.to_string()).fetch_optional(&mut *tx).await?.ok_or(FileManagerError::NotFound)?;
        let current = decode_item(&row)?;
        if next==FileItemState::Completed && current.spec.kind==pab_protocol::ManagedFileKind::Directory {
            let outcome:Option<String>=sqlx::query_scalar("SELECT result FROM fm_directory_finalizations WHERE attempt=?").bind(current.attempt.map(|id|id.to_string())).fetch_optional(&mut *tx).await?.flatten();
            if !outcome.as_deref().map(serde_json::from_str::<pab_protocol::FileManagerResult>).transpose()?.is_some_and(|result|matches!(result,pab_protocol::FileManagerResult::DirectoryMetadata {..})) {return Err(FileManagerError::Conflict);}
        }
        if current.revision != expected_revision
            || !current.state.can_advance_to(next)
            || confirmed.0 < current.confirmed.0
            || confirmed.0 > current.spec.size.0
            || (next == FileItemState::Completed && confirmed != current.spec.size)
        {
            return Err(FileManagerError::Conflict);
        }
        let batch: RequestId = row
            .try_get::<&str, _>("batch")?
            .parse()
            .map_err(|_| FileManagerError::Invalid("invalid stored batch id"))?;
        let total_confirmed: String =
            sqlx::query_scalar("SELECT confirmed FROM fm_batches WHERE id=?")
                .bind(batch.to_string())
                .fetch_one(&mut *tx)
                .await?;
        let total_confirmed = parse_count(&total_confirmed)?
            .checked_add(confirmed.0 - current.confirmed.0)
            .ok_or(FileManagerError::Capacity)?;
        sqlx::query("UPDATE fm_batches SET confirmed=? WHERE id=?")
            .bind(total_confirmed.to_string())
            .bind(batch.to_string())
            .execute(&mut *tx)
            .await?;
        let state = state_text(next)?;
        let released = next.terminal()
            || matches!(
                next,
                FileItemState::Paused | FileItemState::Interrupted | FileItemState::WaitingConflict | FileItemState::WaitingChildren
            );
        sqlx::query("UPDATE fm_items SET state=?,confirmed=?,message=?,worker=?,revision=revision+1,updated=? WHERE id=?")
            .bind(&state).bind(confirmed.0.to_string()).bind(message.map(|s|s.chars().take(2048).collect::<String>()))
            .bind((!released).then(||worker.to_string())).bind(now_ms()).bind(id.to_string()).execute(&mut *tx).await?;
        if let Some(attempt) = current.attempt {
            sqlx::query("UPDATE fm_attempts SET state=?,confirmed=?,ended=? WHERE id=?")
                .bind(&state)
                .bind(confirmed.0.to_string())
                .bind(released.then(now_ms))
                .bind(attempt.to_string())
                .execute(&mut *tx)
                .await?;
        }
        if next == FileItemState::WaitingConflict && message == Some("Destination already exists") {
            sqlx::query("UPDATE fm_items SET state='queued',message=NULL,revision=revision+1 WHERE id=? AND EXISTS(SELECT 1 FROM fm_batch_conflict_choices c WHERE c.batch=fm_items.batch) AND NOT EXISTS(SELECT 1 FROM fm_conflict_choices c WHERE c.item=fm_items.id)").bind(id.to_string()).execute(&mut *tx).await?;
        }
        bump(&mut tx, batch).await?;
        tx.commit().await?;
        self.item(owner, id).await
    }
    pub async fn item(
        &self,
        owner: &str,
        id: RequestId,
    ) -> Result<FileBatchItem, FileManagerError> {
        let row=sqlx::query("SELECT i.*,e.data AS attempt_evidence,df.result AS directory_outcome FROM fm_items i JOIN fm_batches b ON b.id=i.batch LEFT JOIN fm_attempt_evidence e ON e.attempt=i.attempt LEFT JOIN fm_directory_finalizations df ON df.attempt=i.attempt WHERE i.id=? AND b.owner=?")
            .bind(id.to_string()).bind(owner).fetch_optional(&self.pool).await?.ok_or(FileManagerError::NotFound)?;
        decode_item(&row)
    }
    pub async fn items(
        &self,
        owner: &str,
        batch: RequestId,
        after: u32,
        limit: u16,
    ) -> Result<Vec<FileBatchItem>, FileManagerError> {
        if limit == 0 || limit > 256 {
            return Err(FileManagerError::Invalid("invalid item page size"));
        }
        let rows=sqlx::query("SELECT i.*,e.data AS attempt_evidence,df.result AS directory_outcome FROM fm_items i JOIN fm_batches b ON b.id=i.batch LEFT JOIN fm_attempt_evidence e ON e.attempt=i.attempt LEFT JOIN fm_directory_finalizations df ON df.attempt=i.attempt WHERE i.batch=? AND b.owner=? ORDER BY seq LIMIT ? OFFSET ?")
            .bind(batch.to_string()).bind(owner).bind(i64::from(limit)).bind(i64::from(after)).fetch_all(&self.pool).await?;
        rows.iter().map(decode_item).collect()
    }
    pub async fn snapshot(
        &self,
        owner: &str,
        batch: RequestId,
    ) -> Result<BatchSnapshot, FileManagerError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT * FROM fm_batches WHERE id=? AND owner=?")
            .bind(batch.to_string())
            .bind(owner)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(FileManagerError::NotFound)?;
        let mut result = BatchSnapshot {
            spec: serde_json::from_str(row.try_get("spec")?)?,
            intent: serde_json::from_str(&format!("\"{}\"", row.try_get::<&str, _>("intent")?))?,
            phase: String::new(),
            revision: FileByteCount(row.try_get::<i64, _>("revision")? as u64),
            total: FileByteCount(0),
            completed: FileByteCount(0),
            skipped: FileByteCount(0),
            failed: FileByteCount(0),
            cancelled: FileByteCount(0),
            size: FileByteCount(parse_count(row.try_get("total_size")?)?),
            confirmed: FileByteCount(parse_count(row.try_get("confirmed")?)?),
            active_elapsed_ms: FileByteCount(0),
            scan_complete: row.try_get::<i64, _>("scan_complete")? != 0,
            message: row.try_get("message")?,
        };
        let elapsed: Option<String> = sqlx::query_scalar("SELECT total_ms FROM fm_elapsed WHERE batch=?")
            .bind(batch.to_string()).fetch_optional(&mut *tx).await?;
        result.active_elapsed_ms = FileByteCount(parse_count(elapsed.as_deref().unwrap_or("0"))?);
        // The state index supplies counts; progress never reloads every spec.
        let mut active = false;
        let mut unknown = false;
        let mut conflict = false;
        let groups = sqlx::query(
            "SELECT state,COUNT(*) AS count FROM fm_items WHERE batch=? GROUP BY state",
        )
        .bind(batch.to_string())
        .fetch_all(&mut *tx)
        .await?;
        for group in groups {
            let state: FileItemState =
                serde_json::from_str(&format!("\"{}\"", group.try_get::<&str, _>("state")?))?;
            let count = group.try_get::<i64, _>("count")? as u64;
            result.total.0 += count;
            match state {
                FileItemState::Completed => result.completed.0 += count,
                FileItemState::Skipped => result.skipped.0 += count,
                FileItemState::Failed => result.failed.0 += count,
                FileItemState::Cancelled => result.cancelled.0 += count,
                FileItemState::Unconfirmed | FileItemState::Reconciling => unknown = true,
                FileItemState::WaitingConflict => conflict = true,
                _ => active |= state.active(),
            }
        }
        let finished = result.completed.0 + result.skipped.0 + result.failed.0 + result.cancelled.0
            == result.total.0;
        result.phase = if unknown {
            "reconciling"
        } else if result.message.is_some() {
            "failed"
        } else if !result.scan_complete {
            match result.intent {
                FileTransferIntent::Pause => {
                    if row.try_get::<i64, _>("scan_stopped")? != 0 {
                        "paused"
                    } else {
                        "pause_requested"
                    }
                }
                FileTransferIntent::Cancel => {
                    if row.try_get::<i64, _>("scan_stopped")? != 0 {
                        "cancelled"
                    } else {
                        "cancel_requested"
                    }
                }
                _ => "scanning",
            }
        } else if finished {
            if result.failed.0 == result.total.0 && result.total.0 > 0 {
                "failed"
            } else if result.failed.0 > 0 {
                "partial_failed"
            } else if result.cancelled.0 > 0 {
                "cancelled"
            } else if result.skipped.0 > 0 {
                "completed_with_skips"
            } else {
                "completed"
            }
        } else if result.intent == FileTransferIntent::Pause {
            if active { "pause_requested" } else { "paused" }
        } else if result.intent == FileTransferIntent::Cancel {
            "cancel_requested"
        } else if conflict {
            "waiting_conflict"
        } else if active {
            "running"
        } else {
            "queued"
        }
        .into();
        tx.commit().await?;
        Ok(result)
    }
    pub async fn recover_stale(&self, owner: &str) -> Result<u64, FileManagerError> {
        let mut tx = self.write_transaction().await?;
        let rows=sqlx::query("SELECT i.id,i.batch,i.attempt FROM fm_items i JOIN fm_batches b ON b.id=i.batch LEFT JOIN fm_workers w ON w.id=i.worker AND w.owner=b.owner WHERE b.owner=? AND i.worker IS NOT NULL AND (w.id IS NULL OR w.heartbeat<?)")
            .bind(owner).bind(now_ms()-30_000).fetch_all(&mut *tx).await?;
        for row in &rows {
            let id: &str = row.try_get("id")?;
            sqlx::query("UPDATE fm_items SET state='reconciling',revision=revision+1,updated=?,worker=NULL WHERE id=?")
                .bind(now_ms()).bind(id).execute(&mut *tx).await?;
            if let Some(attempt) = row.try_get::<Option<&str>, _>("attempt")? {
                sqlx::query("UPDATE fm_attempts SET state='reconciling' WHERE id=?")
                    .bind(attempt)
                    .execute(&mut *tx)
                    .await?;
            }
            let batch: RequestId = row
                .try_get::<&str, _>("batch")?
                .parse()
                .map_err(|_| FileManagerError::Invalid("invalid stored batch"))?;
            bump(&mut tx, batch).await?;
        }
        tx.commit().await?;
        Ok(rows.len() as u64)
    }
}
async fn bump(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    batch: RequestId,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE fm_batches SET revision=revision+1,updated=? WHERE id=?")
        .bind(now_ms())
        .bind(batch.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}
fn state_text(state: FileItemState) -> Result<String, serde_json::Error> {
    Ok(serde_json::to_string(&state)?.trim_matches('"').into())
}
fn parse_count(value: &str) -> Result<u64, FileManagerError> {
    value
        .parse()
        .map_err(|_| FileManagerError::Invalid("invalid stored byte count"))
}
pub(crate) fn decode_item(row: &sqlx::sqlite::SqliteRow) -> Result<FileBatchItem, FileManagerError> {
    // Public item queries expose the original attempt's durable result, never
    // infer attribute success from a completed byte count or final path stat.
    let evidence=match row.try_get::<Option<&str>,_>("attempt_evidence") {
        Ok(Some(data))=>Some(serde_json::from_str::<crate::AttemptEvidence>(data)?),
        Ok(None)|Err(sqlx::Error::ColumnNotFound(_))=>None,
        Err(error)=>return Err(error.into()),
    };
    let directory_result=match row.try_get::<Option<&str>,_>("directory_outcome") {
        Ok(Some(data))=>match serde_json::from_str::<pab_protocol::FileManagerResult>(data)? {pab_protocol::FileManagerResult::DirectoryMetadata {metadata,..}=>Some(metadata),_=>None},
        Ok(None)|Err(sqlx::Error::ColumnNotFound(_))=>None,
        Err(error)=>return Err(error.into()),
    };
    let metadata_result=directory_result.or_else(||evidence.and_then(|evidence|evidence.local_metadata_result.or_else(||evidence.receipt.and_then(|receipt|receipt.metadata_result))));
    let parse = |value: Option<String>| -> Result<Option<RequestId>, FileManagerError> {
        value
            .map(|s| {
                s.parse()
                    .map_err(|_| FileManagerError::Invalid("invalid stored attempt"))
            })
            .transpose()
    };
    Ok(FileBatchItem {
        metadata_result,
        spec: serde_json::from_str(row.try_get("spec")?)?,
        state: serde_json::from_str(&format!("\"{}\"", row.try_get::<&str, _>("state")?))?,
        revision: FileByteCount(row.try_get::<i64, _>("revision")? as u64),
        confirmed: FileByteCount(
            row.try_get::<&str, _>("confirmed")?
                .parse()
                .map_err(|_| FileManagerError::Invalid("invalid stored offset"))?,
        ),
        attempt: parse(row.try_get("attempt")?)?,
        previous_attempt: parse(row.try_get("previous")?)?,
        message: row.try_get("message")?,
    })
}
