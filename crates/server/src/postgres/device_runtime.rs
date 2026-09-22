use pab_protocol::{DeviceHello, DeviceHelloResult, EndpointKey};

use super::*;

impl PostgresStore {
    pub async fn publish_device_hello(
        &self,
        endpoint_key: EndpointKey,
        hello: &DeviceHello,
    ) -> Result<DeviceHelloResult, StoreError> {
        let execution_context = serde_json::to_string(&hello.execution_context)
            .map_err(|error| StoreError::InvalidData(error.to_string()))?;
        let accepted_at = sqlx::query_scalar::<_, OffsetDateTime>(
            r#"
            INSERT INTO device_runtime (
                tenant_id,
                device_id,
                execution_context,
                environment_revision,
                agent_version,
                observed_at_unix_ms,
                accepted_at
            )
            SELECT $2, $3, $4::jsonb, $5, $6, $7, clock_timestamp()
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
                execution_context = EXCLUDED.execution_context,
                environment_revision = EXCLUDED.environment_revision,
                agent_version = EXCLUDED.agent_version,
                observed_at_unix_ms = EXCLUDED.observed_at_unix_ms,
                accepted_at = clock_timestamp()
            RETURNING accepted_at
            "#,
        )
        .bind(endpoint_key.as_bytes().as_slice())
        .bind(hello.device_ref.tenant_id.as_uuid())
        .bind(hello.device_ref.device_id.as_uuid())
        .bind(execution_context)
        .bind(&hello.execution_context.environment_revision)
        .bind(&hello.agent_version)
        .bind(hello.observed_at_unix_ms)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::NotFound)?;

        Ok(DeviceHelloResult {
            device_ref: hello.device_ref,
            environment_revision: hello.execution_context.environment_revision.clone(),
            accepted_at_unix_ms: support::unix_millis(accepted_at)?,
        })
    }
}
