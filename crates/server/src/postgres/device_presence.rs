use pab_protocol::{DeviceCode, EndpointKey};
use sqlx::Row;

use super::*;

impl PostgresStore {
    pub async fn device_presence(
        &self,
        requester_key: EndpointKey,
        code: DeviceCode,
    ) -> Result<(String, Option<EndpointKey>), StoreError> {
        let row = sqlx::query(
            r#"
            SELECT device.name, target.endpoint_key
            FROM endpoints requester
            JOIN tenants scope
              ON scope.id = requester.tenant_id AND scope.status = 'active'
            JOIN devices device
              ON device.code = $2 AND device.status = 'active'
            LEFT JOIN device_network network
              ON network.tenant_id = device.tenant_id
             AND network.device_id = device.id
            LEFT JOIN endpoints target
              ON target.endpoint_key = network.endpoint_key
             AND target.device_id = device.id
             AND target.owner_kind = 'device'
             AND target.status = 'active'
            WHERE requester.endpoint_key = $1
              AND requester.status = 'active'
              AND (
                  (requester.owner_kind = 'guest' AND scope.kind = 'guest')
                  OR (requester.owner_kind = 'user' AND EXISTS (
                      SELECT 1 FROM memberships member
                      WHERE member.tenant_id = requester.tenant_id
                        AND member.user_id = requester.user_id
                        AND member.status = 'active'
                  ))
              )
            "#,
        )
        .bind(requester_key.as_bytes().as_slice())
        .bind(code.value() as i32)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::NotFound)?;
        let key: Option<Vec<u8>> = row.try_get("endpoint_key")?;
        let key = key
            .map(|value| {
                value
                    .try_into()
                    .map(EndpointKey::new)
                    .map_err(|_| StoreError::InvalidData("endpoint key is not 32 bytes".to_owned()))
            })
            .transpose()?;
        Ok((row.try_get("name")?, key))
    }
}
