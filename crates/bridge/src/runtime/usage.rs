use super::{RuntimeStore, RuntimeStoreError};
use pab_agent_core::account::{AccountClient, AccountError, AccountStore};
use pab_protocol::{RequestId, UsageBatch, UsageCounters};
use sqlx::{Row, Sqlite, Transaction};

async fn add(
    tx: &mut Transaction<'_, Sqlite>,
    scope: &str,
    at: i64,
    counters: &UsageCounters,
) -> Result<(), RuntimeStoreError> {
    let hour = at - at % 3_600_000;
    let old: Option<String> = sqlx::query_scalar(
        "SELECT counters FROM client_usage_accumulators WHERE scope=? AND hour=?",
    )
    .bind(scope)
    .bind(hour)
    .fetch_optional(&mut **tx)
    .await?;
    let mut value: UsageCounters = old
        .map(|v| serde_json::from_str(&v))
        .transpose()?
        .unwrap_or_default();
    value.add(counters);
    sqlx::query("INSERT INTO client_usage_accumulators(scope,hour,counters) VALUES(?,?,?) ON CONFLICT(scope,hour) DO UPDATE SET counters=excluded.counters")
        .bind(scope).bind(hour).bind(serde_json::to_string(&value)?).execute(&mut **tx).await?;
    Ok(())
}

pub(super) async fn finished_transfer(
    tx: &mut Transaction<'_, Sqlite>,
    id: RequestId,
    state: &str,
) -> Result<(), RuntimeStoreError> {
    let row=sqlx::query("SELECT o.direction,o.size,u.user_json,s.origin FROM runtime_operations o JOIN runtime_operation_users u ON u.operation_id=o.id JOIN runtime_operation_origins s ON s.operation_id=o.id WHERE o.id=? AND o.kind='file_transfer'")
        .bind(id.to_string()).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(());
    };
    let user: Option<pab_protocol::UserAttribution> =
        serde_json::from_str(row.try_get("user_json")?)?;
    let Some(user) = user else {
        return Ok(());
    };
    let scope = format!("{}\n{}", row.try_get::<String, _>("origin")?, user.user_id);
    let mut counters = UsageCounters::default();
    match state {
        "completed" => {
            let size = row.try_get::<i64, _>("size")?.max(0) as u64;
            if row.try_get::<String, _>("direction")? == "upload" {
                counters.uploaded_files = 1;
                counters.uploaded_bytes = size;
            } else {
                counters.downloaded_files = 1;
                counters.downloaded_bytes = size;
            }
        }
        "failed" => counters.failed_transfers = 1,
        "cancelled" => counters.cancelled_transfers = 1,
        _ => return Ok(()),
    }
    add(tx, &scope, super::unix_millis(), &counters).await
}

impl RuntimeStore {
    async fn recover_usage_windows(&self) -> Result<(), RuntimeStoreError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let stale=sqlx::query("SELECT c.session_id,c.device_id,c.scope FROM connection_usage c LEFT JOIN runtime_sessions s ON s.id=c.session_id WHERE s.id IS NULL OR s.stopped_at_unix_ms IS NOT NULL OR s.heartbeat_at_unix_ms<?")
            .bind(super::unix_millis()-30_000).fetch_all(&mut *tx).await?;
        for row in stale {
            if let Some(scope) = row.try_get::<Option<String>, _>("scope")? {
                add(
                    &mut tx,
                    &scope,
                    super::unix_millis(),
                    &UsageCounters {
                        incomplete: true,
                        ..Default::default()
                    },
                )
                .await?;
            }
            sqlx::query("DELETE FROM connection_usage WHERE session_id=? AND device_id=?")
                .bind(row.get::<String, _>("session_id"))
                .bind(row.get::<String, _>("device_id"))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub(super) async fn sample_connection(
        &self,
        session: &str,
        device: pab_protocol::DeviceId,
        connection_id: Option<i64>,
        scope: Option<&str>,
    ) -> Result<(), RuntimeStoreError> {
        let now = super::unix_millis();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let previous =
            sqlx::query("SELECT * FROM connection_usage WHERE session_id=? AND device_id=?")
                .bind(session)
                .bind(device.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(previous) = &previous {
            if let Some(old_scope) = previous.try_get::<Option<String>, _>("scope")? {
                let start = previous.try_get::<i64, _>("last_ms")?;
                let elapsed = now.saturating_sub(start).max(0);
                // Do not count time spent asleep or after an unobserved broken connection.
                let end = start + elapsed.min(30_000);
                let boundary = start - start % 3_600_000 + 3_600_000;
                let first = end.min(boundary).saturating_sub(start).max(0) as u64;
                add(
                    &mut tx,
                    &old_scope,
                    start,
                    &UsageCounters {
                        connection_ms: first,
                        incomplete: elapsed > 30_000,
                        ..Default::default()
                    },
                )
                .await?;
                if end > boundary {
                    add(
                        &mut tx,
                        &old_scope,
                        boundary,
                        &UsageCounters {
                            connection_ms: (end - boundary) as u64,
                            ..Default::default()
                        },
                    )
                    .await?;
                }
            }
        }
        if let Some(connection_id) = connection_id {
            if previous
                .as_ref()
                .is_none_or(|r| r.get::<i64, _>("connection_id") != connection_id)
            {
                if let Some(scope) = scope {
                    add(
                        &mut tx,
                        scope,
                        now,
                        &UsageCounters {
                            connections: 1,
                            ..Default::default()
                        },
                    )
                    .await?;
                }
            }
            sqlx::query("INSERT INTO connection_usage(session_id,device_id,connection_id,scope,last_ms) VALUES(?,?,?,?,?) ON CONFLICT(session_id,device_id) DO UPDATE SET connection_id=excluded.connection_id,scope=excluded.scope,last_ms=excluded.last_ms")
                .bind(session).bind(device.to_string()).bind(connection_id).bind(scope).bind(now).execute(&mut *tx).await?;
        } else {
            sqlx::query("DELETE FROM connection_usage WHERE session_id=? AND device_id=?")
                .bind(session)
                .bind(device.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub(super) async fn sync_usage(
        &self,
        client: &AccountClient,
        store: &AccountStore,
    ) -> Result<(), AccountError> {
        self.recover_usage_windows()
            .await
            .map_err(|_| AccountError::Storage)?;
        let state = store.read()?;
        let Some(user) = state.user else {
            return Ok(());
        };
        let token = store
            .token(state.active_slot.as_deref().ok_or(AccountError::Storage)?)?
            .ok_or(AccountError::Storage)?;
        let scope = format!("{}\n{}", client.origin(), user.id);
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|_| AccountError::Storage)?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM client_usage_outbox")
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| AccountError::Storage)?;
        if count < 20_000 {
            let rows = sqlx::query(
                "SELECT hour,counters FROM client_usage_accumulators WHERE scope=? LIMIT ?",
            )
            .bind(&scope)
            .bind((20_000 - count).min(20))
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| AccountError::Storage)?;
            for row in rows {
                let batch = UsageBatch {
                    id: uuid::Uuid::new_v4(),
                    user_id: Some(user.id),
                    hour_unix_ms: row.get("hour"),
                    counters: serde_json::from_str(row.get("counters"))
                        .map_err(|_| AccountError::Storage)?,
                };
                sqlx::query("INSERT INTO client_usage_outbox(id,scope,payload) VALUES(?,?,?)")
                    .bind(batch.id.to_string())
                    .bind(&scope)
                    .bind(serde_json::to_string(&batch).map_err(|_| AccountError::Storage)?)
                    .execute(&mut *tx)
                    .await
                    .map_err(|_| AccountError::Storage)?;
                sqlx::query("DELETE FROM client_usage_accumulators WHERE scope=? AND hour=?")
                    .bind(&scope)
                    .bind(batch.hour_unix_ms)
                    .execute(&mut *tx)
                    .await
                    .map_err(|_| AccountError::Storage)?;
            }
        }
        tx.commit().await.map_err(|_| AccountError::Storage)?;
        let values: Vec<String> = sqlx::query_scalar(
            "SELECT payload FROM client_usage_outbox WHERE scope=? ORDER BY rowid LIMIT 10",
        )
        .bind(&scope)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AccountError::Storage)?;
        for value in values {
            if store.read()?.revision != state.revision {
                return Ok(());
            }
            let batch: UsageBatch =
                serde_json::from_str(&value).map_err(|_| AccountError::Storage)?;
            if batch.valid_at(super::unix_millis()) {
                client.report_usage(&token, &batch).await?;
            } else {
                let mut tx = self
                    .pool
                    .begin_with("BEGIN IMMEDIATE")
                    .await
                    .map_err(|_| AccountError::Storage)?;
                add(
                    &mut tx,
                    &scope,
                    super::unix_millis(),
                    &UsageCounters {
                        incomplete: true,
                        ..Default::default()
                    },
                )
                .await
                .map_err(|_| AccountError::Storage)?;
                tx.commit().await.map_err(|_| AccountError::Storage)?;
            }
            sqlx::query("DELETE FROM client_usage_outbox WHERE id=?")
                .bind(batch.id.to_string())
                .execute(&self.pool)
                .await
                .map_err(|_| AccountError::Storage)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_protocol::{DeviceId, DeviceRef, TenantId, UserAttribution, UserId};
    async fn total(store: &RuntimeStore, scope: &str) -> UsageCounters {
        let rows: Vec<String> =
            sqlx::query_scalar("SELECT counters FROM client_usage_accumulators WHERE scope=?")
                .bind(scope)
                .fetch_all(&store.pool)
                .await
                .unwrap();
        let mut total = UsageCounters::default();
        for value in rows {
            total.add(&serde_json::from_str(&value).unwrap());
        }
        total
    }
    #[tokio::test]
    async fn file_completion_is_once_and_keeps_submission_account() {
        let dir = tempfile::tempdir().unwrap();
        let store = RuntimeStore::open(&dir.path().join("test.db"))
            .await
            .unwrap();
        store.start_session("s").await.unwrap();
        sqlx::query("INSERT INTO runtime_session_servers VALUES('s','https://server-a')")
            .execute(&store.pool)
            .await
            .unwrap();
        let alice = UserAttribution {
            user_id: UserId::new(),
            username: "a".into(),
        };
        let bob = UserAttribution {
            user_id: UserId::new(),
            username: "b".into(),
        };
        sqlx::query("INSERT INTO runtime_session_users VALUES('s',?)")
            .bind(serde_json::to_string(&alice).unwrap())
            .execute(&store.pool)
            .await
            .unwrap();
        let id = RequestId::new();
        let device = DeviceRef {
            device_id: DeviceId::new(),
            tenant_id: TenantId::new(),
        };
        store
            .start_operation(
                id,
                device,
                None,
                "guest",
                "upload",
                "private-path",
                "private-target",
                false,
                Some("s"),
            )
            .await
            .unwrap();
        store.operation_progress(id, 1234, 1234).await.unwrap();
        // Resuming under another session/server must not change the submission origin.
        store.start_session("resume").await.unwrap();
        sqlx::query("INSERT INTO runtime_session_servers VALUES('resume','https://server-b')")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE runtime_operations SET owner_session_id='resume' WHERE id=?")
            .bind(id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE runtime_session_users SET user_json=?")
            .bind(serde_json::to_string(&bob).unwrap())
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(store.finish_operation(id, "completed", None).await.unwrap());
        assert!(!store.finish_operation(id, "completed", None).await.unwrap());
        let counters = total(&store, &format!("https://server-a\n{}", alice.user_id)).await;
        assert_eq!(counters.uploaded_files, 1);
        assert_eq!(counters.uploaded_bytes, 1234);
        assert_eq!(
            total(&store, &format!("https://server-a\n{}", bob.user_id))
                .await
                .uploaded_files,
            0
        );
        let raw: Vec<String> = sqlx::query_scalar("SELECT counters FROM client_usage_accumulators")
            .fetch_all(&store.pool)
            .await
            .unwrap();
        assert!(!raw.join("").contains("private"));
        store.close().await;
    }
    #[tokio::test]
    async fn account_switch_splits_time_without_counting_a_new_connection_and_crash_marks_gap() {
        let dir = tempfile::tempdir().unwrap();
        let store = RuntimeStore::open(&dir.path().join("test.db"))
            .await
            .unwrap();
        store.start_session("s").await.unwrap();
        let device = DeviceId::new();
        store
            .sample_connection("s", device, Some(12), Some("A"))
            .await
            .unwrap();
        sqlx::query("UPDATE connection_usage SET last_ms=last_ms-1000")
            .execute(&store.pool)
            .await
            .unwrap();
        store
            .sample_connection("s", device, Some(12), Some("B"))
            .await
            .unwrap();
        assert_eq!(total(&store, "A").await.connections, 1);
        assert!(total(&store, "A").await.connection_ms >= 1000);
        assert_eq!(total(&store, "B").await.connections, 0);
        sqlx::query("UPDATE connection_usage SET last_ms=last_ms-1000")
            .execute(&store.pool)
            .await
            .unwrap();
        store
            .sample_connection("s", device, None, None)
            .await
            .unwrap();
        assert!(total(&store, "B").await.connection_ms >= 1000);
        store
            .sample_connection("s", device, Some(13), Some("B"))
            .await
            .unwrap();
        assert_eq!(total(&store, "B").await.connections, 1);
        sqlx::query("UPDATE runtime_sessions SET heartbeat_at_unix_ms=0")
            .execute(&store.pool)
            .await
            .unwrap();
        store.recover_usage_windows().await.unwrap();
        assert!(total(&store, "B").await.incomplete);
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM connection_usage")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
        store.close().await;
    }
}
