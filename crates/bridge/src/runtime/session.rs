use std::time::Duration;

use tokio::sync::watch;

use super::store::{RuntimeStore, RuntimeStoreError};

impl RuntimeStore {
    pub async fn start_session(&self, id: &str) -> Result<(), RuntimeStoreError> {
        sqlx::query("INSERT INTO runtime_sessions (id, heartbeat_at_unix_ms) VALUES (?, ?)")
            .bind(id)
            .bind(super::unix_millis())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn heartbeat_session(&self, id: &str) -> Result<(), RuntimeStoreError> {
        sqlx::query(
            "UPDATE runtime_sessions SET heartbeat_at_unix_ms = ? WHERE id = ? AND stopped_at_unix_ms IS NULL",
        )
        .bind(super::unix_millis())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn stop_session(&self, id: &str) -> Result<(), RuntimeStoreError> {
        sqlx::query(
            "UPDATE runtime_sessions SET stopped_at_unix_ms = ? WHERE id = ? AND stopped_at_unix_ms IS NULL",
        )
        .bind(super::unix_millis())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

pub(super) async fn run_session_heartbeat(
    store: RuntimeStore,
    session_id: String,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(error) = store.heartbeat_session(&session_id).await {
                    tracing::warn!(%error, "failed to save Bridge Runtime heartbeat");
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
        }
    }
}
