use super::*;
use crate::domain::DEVICE_CONNECT_CAPABILITY;

impl PostgresStore {
    pub async fn set_device_connect_grant(
        &self,
        actor: UserId,
        tenant_id: TenantId,
        device_id: DeviceId,
        user_id: UserId,
        allowed: bool,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        let role = support::require_active_membership(&mut tx, actor, tenant_id).await?;
        if role == TeamRole::Member {
            return Err(StoreError::PermissionDenied);
        }
        let target_exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM memberships membership
                JOIN devices device ON device.tenant_id = membership.tenant_id
                JOIN tenants tenant ON tenant.id = membership.tenant_id
                JOIN users target_user ON target_user.id = membership.user_id
                WHERE membership.tenant_id = $1
                  AND membership.user_id = $2
                  AND membership.status = 'active'
                  AND device.id = $3
                  AND device.status = 'active'
                  AND tenant.status = 'active'
                  AND target_user.status = 'active'
            )
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(user_id.as_uuid())
        .bind(device_id.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
        if !target_exists {
            return Err(StoreError::NotFound);
        }

        if allowed {
            sqlx::query(
                r#"
                INSERT INTO device_grants (
                    tenant_id,
                    device_id,
                    user_id,
                    capability_bits,
                    granted_by_user_id
                )
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (tenant_id, device_id, user_id) DO UPDATE SET
                    capability_bits = device_grants.capability_bits | EXCLUDED.capability_bits,
                    granted_by_user_id = EXCLUDED.granted_by_user_id,
                    updated_at = clock_timestamp()
                "#,
            )
            .bind(tenant_id.as_uuid())
            .bind(device_id.as_uuid())
            .bind(user_id.as_uuid())
            .bind(DEVICE_CONNECT_CAPABILITY)
            .bind(actor.as_uuid())
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                r#"
                UPDATE device_grants
                SET capability_bits = capability_bits & ~$4,
                    granted_by_user_id = $5,
                    updated_at = clock_timestamp()
                WHERE tenant_id = $1 AND device_id = $2 AND user_id = $3
                "#,
            )
            .bind(tenant_id.as_uuid())
            .bind(device_id.as_uuid())
            .bind(user_id.as_uuid())
            .bind(DEVICE_CONNECT_CAPABILITY)
            .bind(actor.as_uuid())
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                r#"
                DELETE FROM device_grants
                WHERE tenant_id = $1 AND device_id = $2 AND user_id = $3
                  AND capability_bits = 0
                "#,
            )
            .bind(tenant_id.as_uuid())
            .bind(device_id.as_uuid())
            .bind(user_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
