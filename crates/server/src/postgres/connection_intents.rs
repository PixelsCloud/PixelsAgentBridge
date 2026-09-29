use pab_protocol::{DeviceId, EndpointKey};

use super::*;

impl PostgresStore {
    pub async fn delete_expired_connection_intents(&self) -> Result<u64, StoreError> {
        let mut tx = self.pool.begin().await?;
        let deleted =
            sqlx::query("DELETE FROM device_connection_intents WHERE expires_at <= now()")
                .execute(&mut *tx)
                .await?
                .rows_affected();
        if deleted > 0 {
            support::bump_policy_revision(&mut tx).await?;
        }
        tx.commit().await?;
        Ok(deleted)
    }

    pub async fn renew_connection_intent(
        &self,
        operator: EndpointKey,
        device_id: DeviceId,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        let renewed = sqlx::query(
            "UPDATE device_connection_intents \
             SET expires_at = now() + interval '10 minutes' \
             WHERE operator_endpoint_key = $1 AND device_id = $2 \
               AND expires_at > now() \
               AND expires_at < now() + interval '5 minutes'",
        )
        .bind(operator.as_bytes().as_slice())
        .bind(device_id.as_uuid())
        .execute(&mut *tx)
        .await?;
        if renewed.rows_affected() > 0 {
            support::bump_policy_revision(&mut tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
