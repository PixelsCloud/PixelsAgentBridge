use pab_protocol::{
    DEVICE_NETWORK_SCHEMA_VERSION, DeploymentId, DeviceCode, DeviceId, DeviceNetworkSnapshot,
    DeviceRef, EndpointKey, TenantId, UserId,
};

use super::*;
use crate::domain::DEVICE_CONNECT_CAPABILITY;

impl PostgresStore {
    pub async fn resolve_device_code(
        &self,
        requester_key: EndpointKey,
        requester_user: UserId,
        tenant_id: TenantId,
        deployment_id: DeploymentId,
        code: DeviceCode,
    ) -> Result<DeviceRef, StoreError> {
        let device_id: uuid::Uuid = sqlx::query_scalar(
            r#"
            SELECT device.id
            FROM endpoints requester
            JOIN memberships membership
              ON membership.tenant_id = requester.tenant_id
             AND membership.user_id = requester.user_id
             AND membership.status = 'active'
            JOIN tenants tenant
              ON tenant.id = requester.tenant_id AND tenant.status = 'active'
            JOIN devices device
              ON device.tenant_id = requester.tenant_id
             AND device.code = $4 AND device.status = 'active'
            WHERE requester.endpoint_key = $1
              AND requester.tenant_id = $2
              AND requester.user_id = $3
              AND requester.owner_kind = 'user'
              AND requester.status = 'active'
              AND EXISTS (
                  SELECT 1 FROM device_grants grant_row
                  WHERE grant_row.tenant_id = device.tenant_id
                    AND grant_row.device_id = device.id
                    AND grant_row.user_id = requester.user_id
                    AND (grant_row.capability_bits & $5) = $5
              )
            "#,
        )
        .bind(requester_key.as_bytes().as_slice())
        .bind(tenant_id.as_uuid())
        .bind(requester_user.as_uuid())
        .bind(code.value() as i32)
        .bind(DEVICE_CONNECT_CAPABILITY)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::NotFound)?;
        Ok(DeviceRef {
            deployment_id,
            tenant_id,
            device_id: DeviceId::from_uuid(device_id),
        })
    }

    pub async fn device_network_snapshot(
        &self,
        requester_key: EndpointKey,
        requester_user: UserId,
        device_ref: DeviceRef,
    ) -> Result<DeviceNetworkSnapshot, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT network.endpoint_key,
                   network.endpoint_instance_id,
                   network.address_revision,
                   network.relay_urls::text AS relay_urls,
                   network.direct_addresses::text AS direct_addresses,
                   network.observed_at_unix_ms,
                   network.accepted_at
            FROM endpoints requester
            JOIN memberships membership
              ON membership.tenant_id = requester.tenant_id
             AND membership.user_id = requester.user_id
             AND membership.status = 'active'
            JOIN tenants tenant
              ON tenant.id = requester.tenant_id
             AND tenant.status = 'active'
            JOIN deployments deployment
              ON deployment.singleton = true
             AND deployment.id = $5
            JOIN devices device
              ON device.tenant_id = requester.tenant_id
             AND device.id = $4
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
            WHERE requester.endpoint_key = $1
              AND requester.tenant_id = $2
              AND requester.user_id = $3
              AND requester.owner_kind = 'user'
              AND requester.status = 'active'
              AND EXISTS (
                  SELECT 1
                  FROM device_grants grant_row
                  WHERE grant_row.tenant_id = device.tenant_id
                    AND grant_row.device_id = device.id
                    AND grant_row.user_id = requester.user_id
                    AND (grant_row.capability_bits & $6) = $6
              )
            "#,
        )
        .bind(requester_key.as_bytes().as_slice())
        .bind(device_ref.tenant_id.as_uuid())
        .bind(requester_user.as_uuid())
        .bind(device_ref.device_id.as_uuid())
        .bind(device_ref.deployment_id.as_uuid())
        .bind(DEVICE_CONNECT_CAPABILITY)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::NotFound)?;

        let endpoint_key: Vec<u8> = row.try_get("endpoint_key")?;
        let endpoint_key = endpoint_key
            .try_into()
            .map(EndpointKey::new)
            .map_err(|_| StoreError::InvalidData("endpoint key is not 32 bytes".to_owned()))?;
        let address_revision = u64::try_from(row.try_get::<i64, _>("address_revision")?)
            .map_err(support::invalid_number)?;
        let relay_urls = serde_json::from_str(row.try_get::<String, _>("relay_urls")?.as_str())
            .map_err(|error| StoreError::InvalidData(error.to_string()))?;
        let direct_addresses =
            serde_json::from_str(row.try_get::<String, _>("direct_addresses")?.as_str())
                .map_err(|error| StoreError::InvalidData(error.to_string()))?;
        Ok(DeviceNetworkSnapshot {
            schema_version: DEVICE_NETWORK_SCHEMA_VERSION,
            device_ref,
            endpoint_key,
            endpoint_instance_id: pab_protocol::EndpointInstanceId::from_uuid(
                row.try_get("endpoint_instance_id")?,
            ),
            address_revision,
            relay_urls,
            direct_addresses,
            observed_at_unix_ms: row.try_get("observed_at_unix_ms")?,
            accepted_at_unix_ms: support::unix_millis(row.try_get("accepted_at")?)?,
        })
    }
}
