use super::*;
use rand_core::{OsRng, RngCore};

impl PostgresStore {
    pub async fn registered_endpoint(
        &self,
        endpoint_key: EndpointKey,
    ) -> Result<RegisteredEndpoint, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT e.tenant_id, e.owner_kind, e.user_id, e.device_id
            FROM endpoints e
            JOIN tenants tenant ON tenant.id = e.tenant_id AND tenant.status = 'active'
            LEFT JOIN users u ON u.id = e.user_id
            LEFT JOIN devices device
                   ON device.tenant_id = e.tenant_id AND device.id = e.device_id
            WHERE e.endpoint_key = $1
              AND e.status = 'active'
              AND (
                  (e.owner_kind = 'user' AND u.status = 'active')
                  OR
                  (e.owner_kind = 'device' AND device.status = 'active')
                  OR (e.owner_kind = 'guest' AND tenant.kind = 'guest')
              )
            "#,
        )
        .bind(endpoint_key.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::NotFound)?;
        let tenant_id = TenantId::from_uuid(row.try_get("tenant_id")?);
        let owner_kind: String = row.try_get("owner_kind")?;
        let principal = match owner_kind.as_str() {
            "user" => EndpointProofPrincipal::User {
                user_id: UserId::from_uuid(row.try_get("user_id")?),
            },
            "device" => EndpointProofPrincipal::Device {
                device_id: DeviceId::from_uuid(row.try_get("device_id")?),
            },
            "guest" => EndpointProofPrincipal::Guest,
            other => {
                return Err(StoreError::InvalidData(format!(
                    "unknown endpoint owner kind {other}"
                )));
            }
        };
        Ok(RegisteredEndpoint {
            endpoint_key,
            tenant_id,
            principal,
        })
    }

    pub async fn register_user_endpoint(
        &self,
        actor: UserId,
        tenant_id: TenantId,
        endpoint_key: EndpointKey,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        support::require_account_namespace(&mut tx, actor, tenant_id).await?;
        let result = sqlx::query(
            r#"
            INSERT INTO endpoints (endpoint_key, tenant_id, owner_kind, user_id)
            VALUES ($1, $2, 'user', $3)
            "#,
        )
        .bind(endpoint_key.as_bytes().as_slice())
        .bind(tenant_id.as_uuid())
        .bind(actor.as_uuid())
        .execute(&mut *tx)
        .await;
        if let Err(error) = result {
            return Err(support::map_conflict(
                error,
                "endpoint is already registered",
            ));
        }
        support::bump_policy_revision(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn register_device(
        &self,
        actor: UserId,
        tenant_id: TenantId,
        name: &str,
        endpoint_key: EndpointKey,
    ) -> Result<Device, StoreError> {
        let name = support::validate_name(name, "device")?;
        let mut tx = self.pool.begin().await?;
        support::require_account_namespace(&mut tx, actor, tenant_id).await?;
        let kind: String = sqlx::query_scalar("SELECT kind FROM tenants WHERE id = $1")
            .bind(tenant_id.as_uuid())
            .fetch_one(&mut *tx)
            .await?;
        if kind != "personal" {
            return Err(StoreError::PermissionDenied);
        }

        let device_id = DeviceId::new();
        let code = loop {
            let candidate =
                pab_protocol::DeviceCode::new(100_000_000 + OsRng.next_u32() % 900_000_000)
                    .expect("generated nine-digit code");
            let inserted = sqlx::query_scalar::<_, uuid::Uuid>(
                r#"
                INSERT INTO devices (
                    id, tenant_id, owner_tenant_id, code, name, registered_by_user_id
                )
                VALUES ($1, $2, $2, $3, $4, $5)
                ON CONFLICT (code) DO NOTHING
                RETURNING id
                "#,
            )
            .bind(device_id.as_uuid())
            .bind(tenant_id.as_uuid())
            .bind(candidate.value() as i32)
            .bind(&name)
            .bind(actor.as_uuid())
            .fetch_optional(&mut *tx)
            .await?;
            if inserted.is_some() {
                break candidate;
            }
        };
        sqlx::query("INSERT INTO device_accounts(device_id,user_id,revision) VALUES($1,$2,1)")
            .bind(device_id.as_uuid())
            .bind(actor.as_uuid())
            .execute(&mut *tx)
            .await?;
        let endpoint_result = sqlx::query(
            r#"
            INSERT INTO endpoints (endpoint_key, tenant_id, owner_kind, device_id)
            VALUES ($1, $2, 'device', $3)
            "#,
        )
        .bind(endpoint_key.as_bytes().as_slice())
        .bind(tenant_id.as_uuid())
        .bind(device_id.as_uuid())
        .execute(&mut *tx)
        .await;
        if let Err(error) = endpoint_result {
            return Err(support::map_conflict(
                error,
                "endpoint is already registered",
            ));
        }
        support::bump_policy_revision(&mut tx).await?;
        tx.commit().await?;

        Ok(Device {
            id: device_id,
            code,
            tenant_id,
            name,
        })
    }

    pub async fn relay_policy_snapshot(
        &self,
        validity: Duration,
    ) -> Result<RelayPolicySnapshot, StoreError> {
        if validity.is_zero() {
            return Err(StoreError::InvalidInput(
                "relay policy validity must be greater than zero".to_owned(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let settings = sqlx::query("SELECT default_user_mbps, default_guest_mbps, policy_revision FROM server_settings WHERE singleton")
            .fetch_optional(&mut *tx).await?.ok_or_else(|| StoreError::InvalidState("server settings are not initialized".into()))?;
        let defaults = RelayLimitDefaults {
            user_mbps: support::positive_u32(settings.try_get("default_user_mbps")?)?,
            guest_mbps: support::positive_u32(settings.try_get("default_guest_mbps")?)?,
        };
        let user_limits = sqlx::query("SELECT id, relay_limit_mbps FROM users WHERE status = 'active' AND relay_limit_mbps IS NOT NULL ORDER BY id")
            .fetch_all(&mut *tx).await?.into_iter().map(|row| {
                Ok(pab_protocol::UserRelayLimit {
                    user_id: UserId::from_uuid(row.try_get("id")?),
                    mbps: support::positive_u32(row.try_get("relay_limit_mbps")?)?,
                })
            }).collect::<Result<Vec<_>, StoreError>>()?;

        let endpoint_rows = sqlx::query(
            r#"
            SELECT e.endpoint_key, e.tenant_id, e.owner_kind, e.user_id, e.device_id,
                   tenant.kind AS tenant_kind,
                   context.endpoint_key IS NOT NULL AS has_user_context,
                   context_user.id AS context_user_id,
                   COALESCE(device.owner_tenant_id, e.tenant_id) AS relay_tenant_id
            FROM endpoints e
            LEFT JOIN endpoint_user_contexts context ON context.endpoint_key=e.endpoint_key
            LEFT JOIN web_sessions context_session ON context_session.token_hash=context.token_hash
            LEFT JOIN users context_user ON context_user.id=context_session.user_id AND context_user.status='active'
            JOIN tenants tenant ON tenant.id = e.tenant_id AND tenant.status = 'active'
            LEFT JOIN users u ON u.id = e.user_id
            LEFT JOIN devices device
                   ON device.tenant_id = e.tenant_id AND device.id = e.device_id
            WHERE e.status = 'active'
              AND (
                  (e.owner_kind = 'user' AND u.status = 'active')
                  OR
                  (e.owner_kind = 'device' AND device.status = 'active')
                  OR (e.owner_kind = 'guest' AND tenant.kind = 'guest')
              )
            ORDER BY e.endpoint_key
            "#,
        )
        .fetch_all(&mut *tx)
        .await?;
        let endpoints = endpoint_rows
            .into_iter()
            .map(endpoint_policy_from_row)
            .collect::<Result<Vec<_>, StoreError>>()?;

        let intent_rows = sqlx::query(
            "SELECT intent.operator_endpoint_key, intent.device_id, intent.expires_at \
             FROM device_connection_intents intent \
             JOIN endpoints source ON source.endpoint_key = intent.operator_endpoint_key \
               AND source.owner_kind IN ('guest', 'user') AND source.status = 'active' \
             JOIN devices device ON device.id = intent.device_id AND device.status = 'active' \
             WHERE intent.expires_at > now()",
        )
        .fetch_all(&mut *tx)
        .await?;
        let connection_intents = intent_rows
            .into_iter()
            .map(|row| {
                let key: [u8; 32] = row
                    .try_get::<Vec<u8>, _>("operator_endpoint_key")?
                    .try_into()
                    .map_err(|_| {
                        StoreError::InvalidData("endpoint key is not 32 bytes".to_owned())
                    })?;
                Ok(pab_protocol::RelayConnectionIntent {
                    operator_endpoint_key: EndpointKey::new(key),
                    device_id: DeviceId::from_uuid(row.try_get("device_id")?),
                    expires_at_unix_ms: support::unix_millis(row.try_get("expires_at")?)?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;

        let issued_at = OffsetDateTime::now_utc();
        let expires_at = issued_at
            .checked_add(
                time::Duration::try_from(validity)
                    .map_err(|error| StoreError::InvalidInput(error.to_string()))?,
            )
            .ok_or_else(|| StoreError::InvalidInput("relay validity is too large".to_owned()))?;
        let snapshot = RelayPolicySnapshot {
            schema_version: RELAY_POLICY_SCHEMA_VERSION,
            policy_version: u64::try_from(settings.try_get::<i64, _>("policy_revision")?)
                .map_err(support::invalid_number)?,
            issued_at_unix_ms: support::unix_millis(issued_at)?,
            expires_at_unix_ms: support::unix_millis(expires_at)?,
            defaults,
            user_limits,
            endpoints,
            connection_intents,
        };
        snapshot
            .validate()
            .map_err(|error| StoreError::InvalidData(error.to_string()))?;
        tx.commit().await?;
        Ok(snapshot)
    }
}

fn endpoint_policy_from_row(row: sqlx::postgres::PgRow) -> Result<RelayEndpointPolicy, StoreError> {
    let bytes: Vec<u8> = row.try_get("endpoint_key")?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| StoreError::InvalidData("endpoint key is not 32 bytes".to_owned()))?;
    let owner_kind: String = row.try_get("owner_kind")?;
    let owner = if owner_kind != "device" && row.try_get::<bool, _>("has_user_context")? {
        match row.try_get::<Option<uuid::Uuid>, _>("context_user_id")? {
            Some(user_id) => RelayEndpointOwner::User {
                scope: TrafficScope::User {
                    user_id: UserId::from_uuid(user_id),
                },
            },
            None => RelayEndpointOwner::Guest,
        }
    } else {
        match owner_kind.as_str() {
            "user" => {
                let user_id = UserId::from_uuid(row.try_get("user_id")?);
                let scope = TrafficScope::User { user_id };
                RelayEndpointOwner::User { scope }
            }
            "device" => RelayEndpointOwner::Device {
                tenant_id: TenantId::from_uuid(row.try_get("relay_tenant_id")?),
                device_id: DeviceId::from_uuid(row.try_get("device_id")?),
            },
            "guest" => RelayEndpointOwner::Guest,
            other => {
                return Err(StoreError::InvalidData(format!(
                    "unknown endpoint owner kind {other}"
                )));
            }
        }
    };
    Ok(RelayEndpointPolicy {
        endpoint_key: EndpointKey::new(bytes),
        owner,
    })
}
