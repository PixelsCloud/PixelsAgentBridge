use pab_protocol::{AuthorizedDevicePeer, DeviceId, EndpointKey};

use super::*;
use crate::{domain::DEVICE_CONNECT_CAPABILITY, domain::RegisteredEndpoint};

impl PostgresStore {
    pub async fn authorize_device_peer(
        &self,
        device_endpoint: &RegisteredEndpoint,
        device_id: DeviceId,
        peer_endpoint_key: EndpointKey,
    ) -> Result<AuthorizedDevicePeer, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT deployment.id AS deployment_id, peer.user_id
            FROM endpoints device_endpoint
            JOIN devices device
              ON device.tenant_id = device_endpoint.tenant_id
             AND device.id = device_endpoint.device_id
             AND device.status = 'active'
            JOIN tenants tenant
              ON tenant.id = device.tenant_id
             AND tenant.status = 'active'
            JOIN deployments deployment ON deployment.singleton = true
            JOIN endpoints peer
              ON peer.endpoint_key = $4
             AND peer.tenant_id = device.owner_tenant_id
             AND peer.owner_kind = 'user'
             AND peer.status = 'active'
            JOIN users peer_user
              ON peer_user.id = peer.user_id
             AND peer_user.status = 'active'
            JOIN memberships membership
              ON membership.tenant_id = peer.tenant_id
             AND membership.user_id = peer.user_id
             AND membership.status = 'active'
            JOIN device_grants grant_row
              ON grant_row.tenant_id = device.owner_tenant_id
             AND grant_row.device_id = device.id
             AND grant_row.user_id = peer.user_id
             AND (grant_row.capability_bits & $5) = $5
            WHERE device_endpoint.endpoint_key = $1
              AND device_endpoint.tenant_id = $2
              AND device_endpoint.device_id = $3
              AND device_endpoint.owner_kind = 'device'
              AND device_endpoint.status = 'active'
            "#,
        )
        .bind(device_endpoint.endpoint_key.as_bytes().as_slice())
        .bind(device_endpoint.tenant_id.as_uuid())
        .bind(device_id.as_uuid())
        .bind(peer_endpoint_key.as_bytes().as_slice())
        .bind(DEVICE_CONNECT_CAPABILITY)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::NotFound)?;

        Ok(AuthorizedDevicePeer {
            device_ref: pab_protocol::DeviceRef {
                deployment_id: DeploymentId::from_uuid(row.try_get("deployment_id")?),
                tenant_id: device_endpoint.tenant_id,
                device_id,
            },
            peer_endpoint_key,
            operator: pab_protocol::OperatorRef::Account(UserId::from_uuid(
                row.try_get("user_id")?,
            )),
            authorized_at_unix_ms: support::unix_millis(OffsetDateTime::now_utc())?,
        })
    }
}
