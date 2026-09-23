use pab_protocol::{DeviceCode, DeviceId, EndpointKey, TenantId};
use rand_core::{OsRng, RngCore};

use super::*;

impl PostgresStore {
    pub async fn register_unclaimed_device(
        &self,
        tenant_id: TenantId,
        device_id: DeviceId,
        name: &str,
        endpoint_key: EndpointKey,
    ) -> Result<crate::domain::Device, StoreError> {
        let name = support::validate_name(name, "device")?;
        let mut tx = self.pool.begin().await?;
        if let Some(row) = sqlx::query(
            "SELECT device.id, device.tenant_id, device.code, device.name \
             FROM endpoints endpoint JOIN devices device \
               ON device.tenant_id = endpoint.tenant_id AND device.id = endpoint.device_id \
             JOIN tenants tenant ON tenant.id = device.tenant_id \
             WHERE endpoint.endpoint_key = $1 AND endpoint.owner_kind = 'device' \
               AND endpoint.status = 'active' AND tenant.kind = 'unclaimed_device'",
        )
        .bind(endpoint_key.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            return Ok(crate::domain::Device {
                id: DeviceId::from_uuid(row.try_get("id")?),
                code: DeviceCode::new(row.try_get::<i32, _>("code")? as u32)
                    .map_err(|error| StoreError::InvalidData(error.to_string()))?,
                tenant_id: TenantId::from_uuid(row.try_get("tenant_id")?),
                name: row.try_get("name")?,
            });
        }
        sqlx::query("INSERT INTO tenants (id, kind) VALUES ($1, 'unclaimed_device')")
            .bind(tenant_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        let code = loop {
            let candidate = DeviceCode::new(100_000_000 + OsRng.next_u32() % 900_000_000)
                .expect("generated nine-digit device code");
            let inserted = sqlx::query_scalar::<_, uuid::Uuid>(
                "INSERT INTO devices (id, tenant_id, code, name) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT (code) DO NOTHING RETURNING id",
            )
            .bind(device_id.as_uuid())
            .bind(tenant_id.as_uuid())
            .bind(candidate.value() as i32)
            .bind(&name)
            .fetch_optional(&mut *tx)
            .await?;
            if inserted.is_some() {
                break candidate;
            }
        };
        let insert = sqlx::query(
            "INSERT INTO endpoints (endpoint_key, tenant_id, owner_kind, device_id) \
             VALUES ($1, $2, 'device', $3)",
        )
        .bind(endpoint_key.as_bytes().as_slice())
        .bind(tenant_id.as_uuid())
        .bind(device_id.as_uuid())
        .execute(&mut *tx)
        .await;
        if let Err(error) = insert {
            return Err(support::map_conflict(
                error,
                "endpoint is already registered",
            ));
        }
        support::bump_policy_revision(&mut tx).await?;
        tx.commit().await?;
        Ok(crate::domain::Device {
            id: device_id,
            code,
            tenant_id,
            name,
        })
    }

    pub async fn register_guest_endpoint(
        &self,
        tenant_id: TenantId,
        endpoint_key: EndpointKey,
    ) -> Result<TenantId, StoreError> {
        let mut tx = self.pool.begin().await?;
        if let Some(existing) = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT endpoint.tenant_id FROM endpoints endpoint \
             JOIN tenants tenant ON tenant.id = endpoint.tenant_id \
             WHERE endpoint.endpoint_key = $1 AND endpoint.owner_kind = 'guest' \
               AND endpoint.status = 'active' AND tenant.kind = 'guest'",
        )
        .bind(endpoint_key.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            return Ok(TenantId::from_uuid(existing));
        }
        sqlx::query("INSERT INTO tenants (id, kind) VALUES ($1, 'guest')")
            .bind(tenant_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        let insert = sqlx::query(
            "INSERT INTO endpoints (endpoint_key, tenant_id, owner_kind) VALUES ($1, $2, 'guest')",
        )
        .bind(endpoint_key.as_bytes().as_slice())
        .bind(tenant_id.as_uuid())
        .execute(&mut *tx)
        .await;
        if let Err(error) = insert {
            return Err(support::map_conflict(
                error,
                "endpoint is already registered",
            ));
        }
        support::bump_policy_revision(&mut tx).await?;
        tx.commit().await?;
        Ok(tenant_id)
    }
}
