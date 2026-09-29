use pab_protocol::{ClaimId, DeviceCode};

use super::*;

impl PostgresStore {
    pub async fn begin_device_claim(
        &self,
        actor: UserId,
        device_code: DeviceCode,
        owner_tenant_id: TenantId,
    ) -> Result<ClaimId, StoreError> {
        let mut tx = self.pool.begin().await?;
        support::require_active_membership(&mut tx, actor, owner_tenant_id).await?;
        let kind: String = sqlx::query_scalar("SELECT kind FROM tenants WHERE id = $1")
            .bind(owner_tenant_id.as_uuid())
            .fetch_one(&mut *tx)
            .await?;
        if kind != "personal" {
            return Err(StoreError::PermissionDenied);
        }

        let device_id: uuid::Uuid = sqlx::query_scalar(
            r#"
            SELECT id
            FROM devices
            WHERE code = $1
              AND owner_tenant_id IS NULL
              AND status = 'active'
            "#,
        )
        .bind(device_code.value() as i32)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::NotFound)?;

        let claim_id = ClaimId::new();
        sqlx::query(
            r#"
            INSERT INTO device_claim_requests (
                id, device_id, owner_tenant_id, requested_by_user_id, expires_at
            )
            VALUES ($1, $2, $3, $4, now() + interval '10 minutes')
            "#,
        )
        .bind(claim_id.as_uuid())
        .bind(device_id)
        .bind(owner_tenant_id.as_uuid())
        .bind(actor.as_uuid())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(claim_id)
    }

    pub async fn approve_device_claim(
        &self,
        device_endpoint: &RegisteredEndpoint,
        claim_id: ClaimId,
    ) -> Result<(DeviceId, TenantId), StoreError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            r#"
            SELECT request.device_id,
                   request.owner_tenant_id,
                   request.requested_by_user_id
            FROM device_claim_requests request
            JOIN devices device ON device.id = request.device_id
            JOIN endpoints endpoint
              ON endpoint.device_id = device.id
             AND endpoint.tenant_id = device.tenant_id
            JOIN memberships member
              ON member.tenant_id = request.owner_tenant_id
             AND member.user_id = request.requested_by_user_id
             AND member.status = 'active'
            JOIN tenants owner_scope
              ON owner_scope.id = request.owner_tenant_id
             AND owner_scope.kind = 'personal'
             AND owner_scope.status = 'active'
            WHERE request.id = $1
              AND endpoint.endpoint_key = $2
              AND endpoint.owner_kind = 'device'
              AND endpoint.status = 'active'
              AND request.approved_at IS NULL
              AND request.expires_at > now()
              AND device.owner_tenant_id IS NULL
              AND device.status = 'active'
              AND member.role IN ('owner', 'admin')
            FOR UPDATE OF device
            "#,
        )
        .bind(claim_id.as_uuid())
        .bind(device_endpoint.endpoint_key.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::NotFound)?;

        let device_id = DeviceId::from_uuid(row.try_get("device_id")?);
        let owner_tenant_id = TenantId::from_uuid(row.try_get("owner_tenant_id")?);
        let updated = sqlx::query(
            r#"
            UPDATE devices
            SET owner_tenant_id = $2,
                revision = revision + 1,
                updated_at = now()
            WHERE id = $1 AND owner_tenant_id IS NULL
            "#,
        )
        .bind(device_id.as_uuid())
        .bind(owner_tenant_id.as_uuid())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::Conflict("device was already claimed"));
        }

        sqlx::query("UPDATE device_claim_requests SET approved_at = now() WHERE id = $1")
            .bind(claim_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        support::bump_policy_revision(&mut tx).await?;
        tx.commit().await?;
        Ok((device_id, owner_tenant_id))
    }
}
