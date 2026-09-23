use super::*;
use crate::domain::DEVICE_CONNECT_CAPABILITY;
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
            LEFT JOIN memberships m
                   ON m.tenant_id = e.tenant_id AND m.user_id = e.user_id
            LEFT JOIN devices device
                   ON device.tenant_id = e.tenant_id AND device.id = e.device_id
            WHERE e.endpoint_key = $1
              AND e.status = 'active'
              AND (
                  (e.owner_kind = 'user' AND u.status = 'active' AND m.status = 'active')
                  OR
                  (e.owner_kind = 'device' AND device.status = 'active')
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
        support::require_active_membership(&mut tx, actor, tenant_id).await?;
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
        let role = support::require_active_membership(&mut tx, actor, tenant_id).await?;
        if role == TeamRole::Member {
            return Err(StoreError::PermissionDenied);
        }

        let device_id = DeviceId::new();
        let code = loop {
            let candidate =
                pab_protocol::DeviceCode::new(100_000_000 + OsRng.next_u32() % 900_000_000)
                    .expect("generated nine-digit code");
            let inserted = sqlx::query_scalar::<_, uuid::Uuid>(
                r#"
                INSERT INTO devices (id, tenant_id, code, name, registered_by_user_id)
                VALUES ($1, $2, $3, $4, $5)
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
        sqlx::query(
            r#"
            INSERT INTO device_grants (
                tenant_id,
                device_id,
                user_id,
                capability_bits,
                granted_by_user_id
            )
            VALUES ($1, $2, $3, $4, $3)
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(device_id.as_uuid())
        .bind(actor.as_uuid())
        .bind(DEVICE_CONNECT_CAPABILITY)
        .execute(&mut *tx)
        .await?;
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
        let deployment = sqlx::query(
            r#"
            SELECT id, default_team_mbps, default_member_mbps,
                   default_personal_mbps, policy_revision
            FROM deployments
            WHERE singleton = true
            "#,
        )
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::InvalidState("deployment is not initialized".to_owned()))?;

        let defaults = RelayLimitDefaults {
            team_mbps: support::positive_u32(deployment.try_get("default_team_mbps")?)?,
            member_mbps: support::positive_u32(deployment.try_get("default_member_mbps")?)?,
            personal_mbps: support::positive_u32(deployment.try_get("default_personal_mbps")?)?,
        };
        let team_rows = sqlx::query(
            r#"
            SELECT t.tenant_id,
                   COALESCE(t.relay_total_mbps, d.default_team_mbps) AS total_mbps,
                   COALESCE(t.relay_member_mbps, d.default_member_mbps) AS member_mbps
            FROM teams t
            JOIN tenants tenant ON tenant.id = t.tenant_id
            CROSS JOIN deployments d
            WHERE tenant.status = 'active' AND d.singleton = true
            ORDER BY t.tenant_id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        let team_limits = team_rows
            .into_iter()
            .map(|row| {
                Ok(TeamRelayLimits {
                    tenant_id: TenantId::from_uuid(row.try_get("tenant_id")?),
                    total_mbps: support::positive_u32(row.try_get("total_mbps")?)?,
                    member_mbps: support::positive_u32(row.try_get("member_mbps")?)?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;

        let endpoint_rows = sqlx::query(
            r#"
            SELECT e.endpoint_key, e.tenant_id, e.owner_kind, e.user_id, e.device_id,
                   tenant.kind AS tenant_kind
            FROM endpoints e
            JOIN tenants tenant ON tenant.id = e.tenant_id AND tenant.status = 'active'
            LEFT JOIN users u ON u.id = e.user_id
            LEFT JOIN memberships m
                   ON m.tenant_id = e.tenant_id AND m.user_id = e.user_id
            LEFT JOIN devices device
                   ON device.tenant_id = e.tenant_id AND device.id = e.device_id
            WHERE e.status = 'active'
              AND (
                  (e.owner_kind = 'user' AND u.status = 'active' AND m.status = 'active')
                  OR
                  (e.owner_kind = 'device' AND device.status = 'active')
              )
            ORDER BY e.endpoint_key
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        let endpoints = endpoint_rows
            .into_iter()
            .map(endpoint_policy_from_row)
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
            deployment_id: DeploymentId::from_uuid(deployment.try_get("id")?),
            policy_version: u64::try_from(deployment.try_get::<i64, _>("policy_revision")?)
                .map_err(support::invalid_number)?,
            issued_at_unix_ms: support::unix_millis(issued_at)?,
            expires_at_unix_ms: support::unix_millis(expires_at)?,
            defaults,
            team_limits,
            endpoints,
        };
        snapshot
            .validate()
            .map_err(|error| StoreError::InvalidData(error.to_string()))?;
        Ok(snapshot)
    }
}

fn endpoint_policy_from_row(row: sqlx::postgres::PgRow) -> Result<RelayEndpointPolicy, StoreError> {
    let bytes: Vec<u8> = row.try_get("endpoint_key")?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| StoreError::InvalidData("endpoint key is not 32 bytes".to_owned()))?;
    let tenant_id = TenantId::from_uuid(row.try_get("tenant_id")?);
    let owner_kind: String = row.try_get("owner_kind")?;
    let owner = match owner_kind.as_str() {
        "user" => {
            let user_id = UserId::from_uuid(row.try_get("user_id")?);
            let tenant_kind: String = row.try_get("tenant_kind")?;
            let scope = match tenant_kind.as_str() {
                "team" => TrafficScope::Team { tenant_id, user_id },
                "personal" => TrafficScope::Personal { tenant_id, user_id },
                other => {
                    return Err(StoreError::InvalidData(format!(
                        "unknown tenant kind {other}"
                    )));
                }
            };
            RelayEndpointOwner::User { scope }
        }
        "device" => RelayEndpointOwner::Device {
            tenant_id,
            device_id: DeviceId::from_uuid(row.try_get("device_id")?),
        },
        other => {
            return Err(StoreError::InvalidData(format!(
                "unknown endpoint owner kind {other}"
            )));
        }
    };
    Ok(RelayEndpointPolicy {
        endpoint_key: EndpointKey::new(bytes),
        owner,
    })
}
