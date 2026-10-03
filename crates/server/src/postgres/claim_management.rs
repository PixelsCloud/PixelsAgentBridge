use super::*;
use pab_protocol::{ClaimId, DeviceClaimEntry, EndpointProofPrincipal};

impl PostgresStore {
    pub async fn pending_device_claims(
        &self,
        endpoint: &RegisteredEndpoint,
    ) -> Result<Vec<DeviceClaimEntry>, StoreError> {
        if !matches!(endpoint.principal, EndpointProofPrincipal::Device { .. }) {
            return Err(StoreError::PermissionDenied);
        }
        let rows=sqlx::query(r#"SELECT c.id,u.username,(extract(epoch from c.expires_at)*1000)::bigint expires
          FROM device_claim_requests c JOIN users u ON u.id=c.requested_by_user_id AND u.status='active'
          JOIN devices d ON d.id=c.device_id AND d.owner_tenant_id IS NULL AND d.status='active'
          JOIN endpoints e ON e.device_id=d.id AND e.owner_kind='device' AND e.status='active'
          WHERE e.endpoint_key=$1 AND c.resolution='pending' AND c.expires_at>now() ORDER BY c.created_at,c.id LIMIT 20"#)
            .bind(endpoint.endpoint_key.as_bytes().as_slice()).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|r| {
                Ok(DeviceClaimEntry {
                    claim_id: ClaimId::from_uuid(r.try_get("id")?),
                    username: r.try_get("username")?,
                    expires_at_unix_ms: r.try_get("expires")?,
                })
            })
            .collect()
    }

    pub async fn reject_device_claim(
        &self,
        endpoint: &RegisteredEndpoint,
        id: ClaimId,
    ) -> Result<(), StoreError> {
        if !matches!(endpoint.principal, EndpointProofPrincipal::Device { .. }) {
            return Err(StoreError::PermissionDenied);
        }
        let mut tx = self.pool.begin().await?;
        let row=sqlx::query(r#"SELECT device_id,resolution FROM device_claim_requests c WHERE c.id=$1 AND c.resolution IN ('pending','rejected') AND c.expires_at>now()
          AND EXISTS(SELECT 1 FROM endpoints e WHERE e.device_id=c.device_id AND e.endpoint_key=$2 AND e.owner_kind='device' AND e.status='active') FOR UPDATE"#)
            .bind(id.as_uuid()).bind(endpoint.endpoint_key.as_bytes().as_slice()).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            return Err(StoreError::NotFound);
        };
        if row.try_get::<String, _>("resolution")? == "pending" {
            sqlx::query("UPDATE device_claim_requests SET resolution='rejected' WHERE id=$1")
                .bind(id.as_uuid())
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO web_admin_events(actor_id,action,resource_id) VALUES(NULL,'device.claim_rejected',$1)").bind(row.try_get::<uuid::Uuid,_>("device_id")?).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
