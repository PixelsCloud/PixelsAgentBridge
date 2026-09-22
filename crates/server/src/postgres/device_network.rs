use pab_protocol::{DeviceNetworkResult, DeviceNetworkUpdate};

use super::*;

impl PostgresStore {
    pub async fn publish_device_network(
        &self,
        update: &DeviceNetworkUpdate,
    ) -> Result<DeviceNetworkResult, StoreError> {
        let relay_urls = serde_json::to_string(&update.relay_urls)
            .map_err(|error| StoreError::InvalidData(error.to_string()))?;
        let direct_addresses = serde_json::to_string(&update.direct_addresses)
            .map_err(|error| StoreError::InvalidData(error.to_string()))?;
        let address_revision = i64::try_from(update.address_revision)
            .map_err(|error| StoreError::InvalidInput(error.to_string()))?;
        let accepted_at = sqlx::query_scalar::<_, OffsetDateTime>(
            r#"
            INSERT INTO device_network (
                tenant_id,
                device_id,
                endpoint_key,
                endpoint_instance_id,
                address_revision,
                relay_urls,
                direct_addresses,
                observed_at_unix_ms,
                accepted_at
            )
            SELECT $2, $3, $1, $4, $5, $6::jsonb, $7::jsonb, $8, clock_timestamp()
            FROM endpoints endpoint
            JOIN devices device
              ON device.tenant_id = endpoint.tenant_id
             AND device.id = endpoint.device_id
            JOIN tenants tenant ON tenant.id = device.tenant_id
            WHERE endpoint.endpoint_key = $1
              AND endpoint.tenant_id = $2
              AND endpoint.device_id = $3
              AND endpoint.owner_kind = 'device'
              AND endpoint.status = 'active'
              AND device.status = 'active'
              AND tenant.status = 'active'
            ON CONFLICT (tenant_id, device_id) DO UPDATE SET
                endpoint_key = EXCLUDED.endpoint_key,
                endpoint_instance_id = EXCLUDED.endpoint_instance_id,
                address_revision = EXCLUDED.address_revision,
                relay_urls = EXCLUDED.relay_urls,
                direct_addresses = EXCLUDED.direct_addresses,
                observed_at_unix_ms = EXCLUDED.observed_at_unix_ms,
                accepted_at = clock_timestamp()
            WHERE device_network.endpoint_instance_id <> EXCLUDED.endpoint_instance_id
               OR device_network.address_revision <= EXCLUDED.address_revision
            RETURNING accepted_at
            "#,
        )
        .bind(update.endpoint_key.as_bytes().as_slice())
        .bind(update.device_ref.tenant_id.as_uuid())
        .bind(update.device_ref.device_id.as_uuid())
        .bind(update.endpoint_instance_id.as_uuid())
        .bind(address_revision)
        .bind(relay_urls)
        .bind(direct_addresses)
        .bind(update.observed_at_unix_ms)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::NotFound)?;

        Ok(DeviceNetworkResult {
            device_ref: update.device_ref,
            endpoint_instance_id: update.endpoint_instance_id,
            address_revision: update.address_revision,
            accepted_at_unix_ms: support::unix_millis(accepted_at)?,
        })
    }
}
