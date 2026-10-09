//! Bounded durable batches. Callers remove a batch only after a server receipt.
use pab_protocol::UsageBatch;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{path::Path, time::Duration};

#[derive(Clone, Debug)]
pub struct UsageSpool {
    pool: SqlitePool,
}

impl UsageSpool {
    pub async fn open(path: &Path) -> Result<Self, sqlx::Error> {
        crate::ensure_data_parent(path).map_err(sqlx::Error::Io)?;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(Duration::from_secs(5)),
            )
            .await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS usage_outbox(id TEXT PRIMARY KEY,scope TEXT NOT NULL,payload TEXT NOT NULL)").execute(&pool).await?;
        Ok(Self { pool })
    }
    pub async fn push(&self, scope: &str, batch: &UsageBatch) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let payload = serde_json::to_string(batch)
            .map_err(|_| sqlx::Error::Protocol("invalid usage".into()))?;
        let previous: Option<(String, String)> =
            sqlx::query_as("SELECT scope,payload FROM usage_outbox WHERE id=?")
                .bind(batch.id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(previous) = previous {
            if previous != (scope.to_owned(), payload) {
                return Err(sqlx::Error::Protocol("usage batch ID reused".into()));
            }
            return Ok(());
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_outbox")
            .fetch_one(&mut *tx)
            .await?;
        if count >= 20_000 {
            return Err(sqlx::Error::Protocol("usage queue is full".into()));
        }
        sqlx::query("INSERT OR IGNORE INTO usage_outbox(id,scope,payload) VALUES(?,?,?)")
            .bind(batch.id.to_string())
            .bind(scope)
            .bind(
                serde_json::to_string(batch)
                    .map_err(|_| sqlx::Error::Protocol("invalid usage".into()))?,
            )
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn pending(&self, scope: &str) -> Result<Vec<UsageBatch>, sqlx::Error> {
        let values: Vec<String> = sqlx::query_scalar(
            "SELECT payload FROM usage_outbox WHERE scope=? ORDER BY rowid LIMIT 100",
        )
        .bind(scope)
        .fetch_all(&self.pool)
        .await?;
        values
            .into_iter()
            .map(|v| {
                serde_json::from_str(&v)
                    .map_err(|_| sqlx::Error::Protocol("invalid stored usage".into()))
            })
            .collect()
    }
    pub async fn acknowledge(&self, id: impl std::fmt::Display) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM usage_outbox WHERE id=?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn sqlite_full_preserves_unacknowledged_batches() {
        let dir = tempfile::tempdir().unwrap();
        let spool = UsageSpool::open(&dir.path().join("full-pages.db")).await.unwrap();
        let batch = UsageBatch { id: pab_protocol::RequestId::new().as_uuid(), user_id: None, hour_unix_ms: 0, counters: Default::default() };
        spool.push("A", &batch).await.unwrap();
        let pages: i64 = sqlx::query_scalar("PRAGMA page_count").fetch_one(&spool.pool).await.unwrap();
        sqlx::query(&format!("PRAGMA max_page_count={pages}")).execute(&spool.pool).await.unwrap();
        let next = UsageBatch { id: pab_protocol::RequestId::new().as_uuid(), ..batch.clone() };
        let error = spool.push(&"B".repeat(65_536), &next).await.unwrap_err();
        assert_eq!(error.as_database_error().unwrap().code().as_deref(), Some("13"));
        assert_eq!(spool.pending("A").await.unwrap()[0].id, batch.id);
        spool.pool.close().await;
    }
    #[tokio::test]
    async fn full_queue_rejects_new_batches_but_allows_retry_and_recovers_after_ack() {
        let dir = tempfile::tempdir().unwrap();
        let spool = UsageSpool::open(&dir.path().join("full.db")).await.unwrap();
        let batch = UsageBatch { id: pab_protocol::RequestId::new().as_uuid(), user_id: None, hour_unix_ms: 0, counters: Default::default() };
        spool.push("A", &batch).await.unwrap();
        sqlx::query("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<19999) INSERT INTO usage_outbox(id,scope,payload) SELECT 'fixture-'||i,'B','{}' FROM n").execute(&spool.pool).await.unwrap();
        spool.push("A", &batch).await.unwrap();
        let next = UsageBatch { id: pab_protocol::RequestId::new().as_uuid(), ..batch.clone() };
        assert!(spool.push("A", &next).await.is_err());
        assert_eq!(spool.pending("A").await.unwrap().len(), 1);
        spool.acknowledge(batch.id).await.unwrap();
        spool.push("A", &next).await.unwrap();
        assert_eq!(spool.pending("A").await.unwrap()[0].id, next.id);
        spool.pool.close().await;
    }
    #[tokio::test]
    async fn immutable_batches_survive_restart_and_stay_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.db");
        let spool = UsageSpool::open(&path).await.unwrap();
        let mut batch = UsageBatch {
            id: pab_protocol::RequestId::new().as_uuid(),
            user_id: None,
            hour_unix_ms: 0,
            counters: Default::default(),
        };
        spool.push("A", &batch).await.unwrap();
        spool.push("A", &batch).await.unwrap();
        assert!(spool.push("B", &batch).await.is_err());
        batch.counters.relay_upload_bytes = 1;
        assert!(spool.push("A", &batch).await.is_err());
        spool.pool.close().await;
        let reopened = UsageSpool::open(&path).await.unwrap();
        assert!(reopened.pending("B").await.unwrap().is_empty());
        let pending = reopened.pending("A").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].counters.relay_upload_bytes, 0);
        reopened.acknowledge(batch.id).await.unwrap();
        assert!(reopened.pending("A").await.unwrap().is_empty());
        reopened.pool.close().await;
    }
}
