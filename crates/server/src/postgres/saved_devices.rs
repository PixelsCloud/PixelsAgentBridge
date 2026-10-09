use super::*;

impl PostgresStore {
    /// Called only through the authenticated target's control session after password verification.
    pub async fn record_authenticated_device(
        &self,
        peer: &pab_protocol::AuthorizedDevicePeer,
    ) -> Result<bool, StoreError> {
        let Some(user) = &peer.user_context.user else {
            return Ok(false);
        };
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO user_catalog_versions(user_id) VALUES($1) ON CONFLICT DO NOTHING")
            .bind(user.user_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT user_id FROM user_catalog_versions WHERE user_id=$1 FOR UPDATE")
            .bind(user.user_id.as_uuid())
            .fetch_one(&mut *tx)
            .await?;
        let existing=sqlx::query("SELECT verified_until>now()+interval '240 seconds' AS fresh FROM user_saved_devices WHERE user_id=$1 AND device_id=$2")
            .bind(user.user_id.as_uuid()).bind(peer.device_ref.device_id.as_uuid()).fetch_optional(&mut *tx).await?;
        if existing.as_ref().is_some_and(|r| r.get::<bool, _>("fresh")) {
            return Ok(false);
        }
        if existing.is_some() {
            let unchanged=sqlx::query("UPDATE user_saved_devices s SET verified_until=now()+interval '5 minutes' FROM devices d LEFT JOIN device_runtime r ON r.device_id=d.id WHERE s.user_id=$1 AND s.device_id=$2 AND d.id=s.device_id AND s.saved_name=d.name AND s.saved_system IS NOT DISTINCT FROM r.execution_context->>'os_family'")
                .bind(user.user_id.as_uuid()).bind(peer.device_ref.device_id.as_uuid()).execute(&mut *tx).await?.rows_affected();
            if unchanged == 1 {
                tx.commit().await?;
                return Ok(false);
            }
        }
        let revision:i64=sqlx::query_scalar("UPDATE user_catalog_versions SET revision=revision+1 WHERE user_id=$1 RETURNING revision")
            .bind(user.user_id.as_uuid()).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO user_saved_devices(user_id,device_id,revision,saved_name,saved_system,verified_until) SELECT $1,d.id,$3,d.name,r.execution_context->>'os_family',now()+interval '5 minutes' FROM devices d LEFT JOIN device_runtime r ON r.device_id=d.id WHERE d.id=$2 ON CONFLICT(user_id,device_id) DO UPDATE SET revision=excluded.revision,saved_name=excluded.saved_name,saved_system=excluded.saved_system,verified_until=excluded.verified_until,updated_at=now()")
            .bind(user.user_id.as_uuid()).bind(peer.device_ref.device_id.as_uuid()).bind(revision).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }
}
