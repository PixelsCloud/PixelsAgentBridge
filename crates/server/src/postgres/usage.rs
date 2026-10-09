use super::*;
use pab_protocol::{UsageBatch, UsageCounters};

impl PostgresStore {
    pub async fn record_usage(
        &self,
        source_kind: &str,
        source: &str,
        batch: &UsageBatch,
    ) -> Result<(), StoreError> {
        let now = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
        if !batch.valid_at(now) || !matches!(source_kind, "client" | "relay") {
            return Err(StoreError::InvalidInput("invalid usage batch".into()));
        }
        let encoded = serde_json::to_vec(batch)
            .map_err(|_| StoreError::InvalidInput("invalid usage".into()))?;
        let digest = blake3::hash(&encoded).to_hex().to_string();
        let subject = batch
            .user_id
            .map(|v| v.to_string())
            .unwrap_or_else(|| "guest".into());
        let source = format!("{source_kind}:{source}");
        let mut tx = self.pool.begin().await?;
        let inserted=sqlx::query("INSERT INTO usage_receipts(source,batch_id,digest,hour_unix_ms) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(&source).bind(batch.id).bind(&digest).bind(batch.hour_unix_ms).execute(&mut *tx).await?.rows_affected();
        if inserted == 0 {
            let previous: String = sqlx::query_scalar(
                "SELECT digest FROM usage_receipts WHERE source=$1 AND batch_id=$2",
            )
            .bind(&source)
            .bind(batch.id)
            .fetch_one(&mut *tx)
            .await?;
            if previous != digest {
                return Err(StoreError::Conflict("usage batch ID reused"));
            }
            return Ok(());
        }
        for (table, bucket) in [
            ("usage_hourly", batch.hour_unix_ms),
            (
                "usage_daily",
                batch.hour_unix_ms - batch.hour_unix_ms % 86_400_000,
            ),
            ("usage_lifetime", 0),
        ] {
            sqlx::query(&format!("INSERT INTO {table}(source_kind,subject,hour_unix_ms,counters) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING"))
                .bind(source_kind).bind(&subject).bind(bucket).bind(serde_json::to_value(UsageCounters::default()).unwrap()).execute(&mut *tx).await?;
            let value:serde_json::Value=sqlx::query_scalar(&format!("SELECT counters FROM {table} WHERE source_kind=$1 AND subject=$2 AND hour_unix_ms=$3 FOR UPDATE"))
                .bind(source_kind).bind(&subject).bind(bucket).fetch_one(&mut *tx).await?;
            let mut counters: UsageCounters = serde_json::from_value(value)
                .map_err(|_| StoreError::InvalidData("invalid usage counters".into()))?;
            counters.add(&batch.counters);
            sqlx::query(&format!("UPDATE {table} SET counters=$1,updated_at=now() WHERE source_kind=$2 AND subject=$3 AND hour_unix_ms=$4"))
                .bind(serde_json::to_value(counters).unwrap()).bind(source_kind).bind(&subject).bind(bucket).execute(&mut *tx).await?;
        }
        // Older batches are rejected before deduplication, so pruning cannot count them again.
        sqlx::query("DELETE FROM usage_receipts WHERE hour_unix_ms<$1")
            .bind(now - 31 * 86_400_000)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM usage_hourly WHERE hour_unix_ms<$1")
            .bind(now - 90 * 86_400_000)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM usage_daily WHERE hour_unix_ms<$1")
            .bind(now - 730 * 86_400_000)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
