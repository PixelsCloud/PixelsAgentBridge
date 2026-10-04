use pab_protocol::{
    AuthorizedDevicePeer, DEVICE_NETWORK_SCHEMA_VERSION, DeviceCode, DeviceId,
    DeviceNetworkSnapshot, DeviceRef, EndpointKey, OperatorRef, TenantId,
};
use sqlx::Row;

use super::*;

impl PostgresStore {
    pub async fn guest_resolve_device_code(
        &self,
        guest_key: EndpointKey,
        code: DeviceCode,
    ) -> Result<DeviceRef, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT device.id, device.tenant_id
            FROM endpoints guest
            JOIN tenants scope
              ON scope.id = guest.tenant_id
             AND scope.kind = 'guest'
             AND scope.status = 'active'
            CROSS JOIN devices device
            WHERE guest.endpoint_key = $1
              AND guest.owner_kind = 'guest'
              AND guest.status = 'active'
              AND device.code = $2
              AND device.status = 'active'
            "#,
        )
        .bind(guest_key.as_bytes().as_slice())
        .bind(code.value() as i32)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::NotFound)?;
        let device_id = DeviceId::from_uuid(row.try_get("id")?);
        let tenant_id = TenantId::from_uuid(row.try_get("tenant_id")?);
        Ok(DeviceRef {
            tenant_id,
            device_id,
        })
    }

    pub async fn guest_device_network_snapshot(
        &self,
        guest_key: EndpointKey,
        device_ref: DeviceRef,
    ) -> Result<DeviceNetworkSnapshot, StoreError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            r#"
            SELECT network.endpoint_key,
                   network.endpoint_instance_id,
                   network.address_revision,
                   network.relay_urls::text AS relay_urls,
                   network.direct_addresses::text AS direct_addresses,
                   network.observed_at_unix_ms,
                   network.accepted_at
            FROM endpoints guest
            JOIN tenants scope
              ON scope.id = guest.tenant_id
             AND scope.kind = 'guest'
             AND scope.status = 'active'
            JOIN devices device
              ON device.id = $2
             AND device.status = 'active'
            JOIN device_network network
              ON network.tenant_id = device.tenant_id
             AND network.device_id = device.id
            JOIN endpoints target
              ON target.endpoint_key = network.endpoint_key
             AND target.tenant_id = device.tenant_id
             AND target.device_id = device.id
             AND target.owner_kind = 'device'
             AND target.status = 'active'
            WHERE guest.endpoint_key = $1
              AND guest.owner_kind = 'guest'
              AND guest.status = 'active'
              AND device.tenant_id = $3
            "#,
        )
        .bind(guest_key.as_bytes().as_slice())
        .bind(device_ref.device_id.as_uuid())
        .bind(device_ref.tenant_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::NotFound)?;
        let endpoint_key: [u8; 32] = row
            .try_get::<Vec<u8>, _>("endpoint_key")?
            .try_into()
            .map_err(|_| StoreError::InvalidData("endpoint key is not 32 bytes".to_owned()))?;
        let snapshot = DeviceNetworkSnapshot {
            schema_version: DEVICE_NETWORK_SCHEMA_VERSION,
            device_ref,
            endpoint_key: EndpointKey::new(endpoint_key),
            endpoint_instance_id: pab_protocol::EndpointInstanceId::from_uuid(
                row.try_get("endpoint_instance_id")?,
            ),
            address_revision: u64::try_from(row.try_get::<i64, _>("address_revision")?)
                .map_err(support::invalid_number)?,
            relay_urls: serde_json::from_str(row.try_get::<String, _>("relay_urls")?.as_str())
                .map_err(|error| StoreError::InvalidData(error.to_string()))?,
            direct_addresses: serde_json::from_str(
                row.try_get::<String, _>("direct_addresses")?.as_str(),
            )
            .map_err(|error| StoreError::InvalidData(error.to_string()))?,
            observed_at_unix_ms: row.try_get("observed_at_unix_ms")?,
            accepted_at_unix_ms: support::unix_millis(row.try_get("accepted_at")?)?,
        };
        connection_intents::grant_device_connection(&mut tx, guest_key, device_ref.device_id)
            .await?;
        tx.commit().await?;
        Ok(snapshot)
    }

    pub async fn authorize_guest_device_peer(
        &self,
        device_endpoint: &RegisteredEndpoint,
        device_id: DeviceId,
        guest_key: EndpointKey,
    ) -> Result<AuthorizedDevicePeer, StoreError> {
        sqlx::query_scalar::<_, bool>(
            r#"
            SELECT true
            FROM endpoints device_endpoint
            JOIN devices device
              ON device.tenant_id = device_endpoint.tenant_id
             AND device.id = device_endpoint.device_id
             AND device.status = 'active'
            JOIN device_connection_intents intent
              ON intent.device_id = device.id
             AND intent.expires_at > now()
            JOIN endpoints guest
              ON guest.endpoint_key = intent.operator_endpoint_key
             AND guest.owner_kind = 'guest'
             AND guest.status = 'active'
            WHERE device_endpoint.endpoint_key = $1
              AND device_endpoint.device_id = $2
              AND device_endpoint.owner_kind = 'device'
              AND device_endpoint.status = 'active'
              AND guest.endpoint_key = $3
            "#,
        )
        .bind(device_endpoint.endpoint_key.as_bytes().as_slice())
        .bind(device_id.as_uuid())
        .bind(guest_key.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::NotFound)?;
        Ok(AuthorizedDevicePeer {
            device_ref: DeviceRef {
                tenant_id: device_endpoint.tenant_id,
                device_id,
            },
            peer_endpoint_key: guest_key,
            operator: OperatorRef::Guest {
                guest_endpoint_key: guest_key,
            },
            authorized_at_unix_ms: support::unix_millis(OffsetDateTime::now_utc())?,
        })
    }
}
