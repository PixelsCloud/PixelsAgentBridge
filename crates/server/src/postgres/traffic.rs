use super::*;

impl PostgresStore {
    pub async fn endpoint_user_context(
        &self,
        endpoint: pab_protocol::EndpointKey,
    ) -> Result<pab_protocol::EndpointUserContext, StoreError> {
        let row = sqlx::query("SELECT c.revision,u.id,u.username,s.policy_revision FROM server_settings s LEFT JOIN endpoint_user_contexts c ON c.endpoint_key=$1 LEFT JOIN web_sessions login ON login.token_hash=c.token_hash LEFT JOIN users u ON u.id=login.user_id AND u.status='active' WHERE s.singleton")
            .bind(endpoint.as_bytes().as_slice()).fetch_one(&self.pool).await?;
        let user_id: Option<uuid::Uuid> = row.try_get("id")?;
        Ok(pab_protocol::EndpointUserContext {
            revision: row.try_get::<Option<i64>, _>("revision")?.unwrap_or(0) as u64,
            user: user_id
                .map(|id| {
                    Ok::<_, StoreError>(pab_protocol::UserAttribution {
                        user_id: UserId::from_uuid(id),
                        username: row.try_get("username")?,
                    })
                })
                .transpose()?,
            policy_version: row.try_get::<i64, _>("policy_revision")? as u64,
        })
    }
    pub async fn list_traffic_scopes(
        &self,
        user_id: UserId,
        personal_tenant_id: TenantId,
    ) -> Result<pab_protocol::TrafficScopeOptions, StoreError> {
        let mbps: i32 = sqlx::query_scalar(
            "SELECT u.relay_limit_mbps FROM users u CROSS JOIN server_settings s WHERE u.id=$1 AND u.status='active' AND s.singleton",
        ).bind(user_id.as_uuid()).fetch_one(&self.pool).await?;
        Ok(pab_protocol::TrafficScopeOptions {
            personal_tenant_id,
            default_tenant_id: personal_tenant_id,
            personal_mbps: support::positive_u32(mbps)?,
        })
    }
}
