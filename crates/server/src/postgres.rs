use std::time::Duration;

use pab_protocol::{
    DeploymentId, DeviceId, EndpointKey, RELAY_POLICY_SCHEMA_VERSION, RelayEndpointOwner,
    RelayEndpointPolicy, RelayLimitDefaults, RelayPolicySnapshot, TeamRelayLimits, TenantId,
    TrafficScope, UserId,
};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::{Account, AccountCredential, Device, Team, TeamInvitation, TeamRole};

#[derive(Debug, Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn connect(database_url: &str, max_connections: u32) -> Result<Self, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(database_url)
            .await?;
        Ok(Self::from_pool(pool))
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn migrate(&self) -> Result<(), StoreError> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    pub async fn initialize_deployment(
        &self,
        requested_id: DeploymentId,
        defaults: RelayLimitDefaults,
    ) -> Result<DeploymentId, StoreError> {
        defaults
            .validate()
            .map_err(|error| StoreError::InvalidInput(error.to_string()))?;
        let row = sqlx::query(
            r#"
            INSERT INTO deployments (
                id, default_team_mbps, default_member_mbps, default_personal_mbps
            )
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (singleton) DO UPDATE SET singleton = deployments.singleton
            RETURNING id
            "#,
        )
        .bind(requested_id.as_uuid())
        .bind(i32::try_from(defaults.team_mbps).map_err(invalid_number)?)
        .bind(i32::try_from(defaults.member_mbps).map_err(invalid_number)?)
        .bind(i32::try_from(defaults.personal_mbps).map_err(invalid_number)?)
        .fetch_one(&self.pool)
        .await?;
        Ok(DeploymentId::from_uuid(row.try_get("id")?))
    }

    pub(crate) async fn register_account(
        &self,
        username: &str,
        username_key: &str,
        password_hash: &str,
    ) -> Result<Account, StoreError> {
        let user_id = UserId::new();
        let tenant_id = TenantId::new();
        let mut tx = self.pool.begin().await?;

        let insert = sqlx::query(
            r#"
            INSERT INTO users (id, username, username_key, password_hash)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(user_id.as_uuid())
        .bind(username)
        .bind(username_key)
        .bind(password_hash)
        .execute(&mut *tx)
        .await;
        if let Err(error) = insert {
            return Err(map_conflict(error, "username is already registered"));
        }

        sqlx::query(
            r#"
            INSERT INTO tenants (id, kind, created_by_user_id)
            VALUES ($1, 'personal', $2)
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO personal_tenants (tenant_id, user_id) VALUES ($1, $2)")
            .bind(tenant_id.as_uuid())
            .bind(user_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO memberships (tenant_id, user_id, role)
            VALUES ($1, $2, 'owner')
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(Account {
            id: user_id,
            username: username.to_owned(),
            personal_tenant_id: tenant_id,
        })
    }

    pub(crate) async fn account_credential(
        &self,
        username_key: &str,
    ) -> Result<Option<AccountCredential>, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT u.id, u.username, u.password_hash, pt.tenant_id
            FROM users u
            JOIN personal_tenants pt ON pt.user_id = u.id
            WHERE u.username_key = $1 AND u.status = 'active'
            "#,
        )
        .bind(username_key)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| {
            Ok(AccountCredential {
                account: Account {
                    id: UserId::from_uuid(row.try_get("id")?),
                    username: row.try_get("username")?,
                    personal_tenant_id: TenantId::from_uuid(row.try_get("tenant_id")?),
                },
                password_hash: row.try_get("password_hash")?,
            })
        })
        .transpose()
    }

    pub async fn create_team(&self, actor: UserId, name: &str) -> Result<Team, StoreError> {
        let name = validate_name(name, "Team")?;
        let tenant_id = TenantId::new();
        let mut tx = self.pool.begin().await?;
        require_active_user(&mut tx, actor).await?;

        sqlx::query("INSERT INTO tenants (id, kind, created_by_user_id) VALUES ($1, 'team', $2)")
            .bind(tenant_id.as_uuid())
            .bind(actor.as_uuid())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO teams (tenant_id, name) VALUES ($1, $2)")
            .bind(tenant_id.as_uuid())
            .bind(&name)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO memberships (tenant_id, user_id, role) VALUES ($1, $2, 'owner')")
            .bind(tenant_id.as_uuid())
            .bind(actor.as_uuid())
            .execute(&mut *tx)
            .await?;
        bump_policy_revision(&mut tx).await?;
        tx.commit().await?;

        Ok(Team { tenant_id, name })
    }

    pub async fn invite_team_member(
        &self,
        actor: UserId,
        tenant_id: TenantId,
        invited_user: UserId,
        role: TeamRole,
        expires_at: OffsetDateTime,
    ) -> Result<TeamInvitation, StoreError> {
        if role == TeamRole::Owner {
            return Err(StoreError::InvalidInput(
                "owner transfer is a separate operation".to_owned(),
            ));
        }
        if expires_at <= OffsetDateTime::now_utc() {
            return Err(StoreError::InvalidInput(
                "invitation expiration must be in the future".to_owned(),
            ));
        }

        let mut tx = self.pool.begin().await?;
        let actor_role = require_team_manager(&mut tx, actor, tenant_id).await?;
        if actor_role == TeamRole::Admin && role != TeamRole::Member {
            return Err(StoreError::PermissionDenied);
        }
        require_active_user(&mut tx, invited_user).await?;

        let invitation_id = Uuid::new_v4();
        let result = sqlx::query(
            r#"
            INSERT INTO team_invitations (
                id, tenant_id, invited_user_id, invited_by_user_id, role, expires_at
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(invitation_id)
        .bind(tenant_id.as_uuid())
        .bind(invited_user.as_uuid())
        .bind(actor.as_uuid())
        .bind(role.as_db())
        .bind(expires_at)
        .execute(&mut *tx)
        .await;
        if let Err(error) = result {
            return Err(map_conflict(error, "a pending invitation already exists"));
        }
        tx.commit().await?;

        Ok(TeamInvitation {
            id: invitation_id,
            tenant_id,
            invited_user_id: invited_user,
            role,
            expires_at,
        })
    }

    pub async fn accept_team_invitation(
        &self,
        actor: UserId,
        invitation_id: Uuid,
    ) -> Result<TenantId, StoreError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            r#"
            SELECT tenant_id, role, status, expires_at
            FROM team_invitations
            WHERE id = $1 AND invited_user_id = $2
            FOR UPDATE
            "#,
        )
        .bind(invitation_id)
        .bind(actor.as_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::NotFound)?;
        let status: String = row.try_get("status")?;
        let expires_at: OffsetDateTime = row.try_get("expires_at")?;
        if status != "pending" || expires_at <= OffsetDateTime::now_utc() {
            return Err(StoreError::InvalidState(
                "invitation is no longer active".to_owned(),
            ));
        }
        let tenant_id = TenantId::from_uuid(row.try_get("tenant_id")?);
        let role: String = row.try_get("role")?;

        sqlx::query(
            r#"
            INSERT INTO memberships (tenant_id, user_id, role)
            VALUES ($1, $2, $3)
            ON CONFLICT (tenant_id, user_id) DO UPDATE SET
                role = EXCLUDED.role,
                status = 'active',
                revision = memberships.revision + 1,
                updated_at = now()
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(actor.as_uuid())
        .bind(role)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE team_invitations SET status = 'accepted', responded_at = now() WHERE id = $1",
        )
        .bind(invitation_id)
        .execute(&mut *tx)
        .await?;
        bump_policy_revision(&mut tx).await?;
        tx.commit().await?;
        Ok(tenant_id)
    }

    pub async fn register_user_endpoint(
        &self,
        actor: UserId,
        tenant_id: TenantId,
        endpoint_key: EndpointKey,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        require_active_membership(&mut tx, actor, tenant_id).await?;
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
            return Err(map_conflict(error, "endpoint is already registered"));
        }
        bump_policy_revision(&mut tx).await?;
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
        let name = validate_name(name, "device")?;
        let mut tx = self.pool.begin().await?;
        let role = require_active_membership(&mut tx, actor, tenant_id).await?;
        if role == TeamRole::Member {
            return Err(StoreError::PermissionDenied);
        }

        let device_id = DeviceId::new();
        sqlx::query(
            r#"
            INSERT INTO devices (id, tenant_id, name, registered_by_user_id)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(device_id.as_uuid())
        .bind(tenant_id.as_uuid())
        .bind(&name)
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
            return Err(map_conflict(error, "endpoint is already registered"));
        }
        bump_policy_revision(&mut tx).await?;
        tx.commit().await?;

        Ok(Device {
            id: device_id,
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
            team_mbps: positive_u32(deployment.try_get("default_team_mbps")?)?,
            member_mbps: positive_u32(deployment.try_get("default_member_mbps")?)?,
            personal_mbps: positive_u32(deployment.try_get("default_personal_mbps")?)?,
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
                    total_mbps: positive_u32(row.try_get("total_mbps")?)?,
                    member_mbps: positive_u32(row.try_get("member_mbps")?)?,
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
                .map_err(invalid_number)?,
            issued_at_unix_ms: unix_millis(issued_at)?,
            expires_at_unix_ms: unix_millis(expires_at)?,
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

async fn require_active_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: UserId,
) -> Result<(), StoreError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND status = 'active')",
    )
    .bind(user_id.as_uuid())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(StoreError::NotFound)
    }
}

async fn require_active_membership(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: UserId,
    tenant_id: TenantId,
) -> Result<TeamRole, StoreError> {
    let role = sqlx::query_scalar::<_, String>(
        r#"
        SELECT m.role
        FROM memberships m
        JOIN users u ON u.id = m.user_id
        JOIN tenants t ON t.id = m.tenant_id
        WHERE m.tenant_id = $1 AND m.user_id = $2
          AND m.status = 'active' AND u.status = 'active' AND t.status = 'active'
        "#,
    )
    .bind(tenant_id.as_uuid())
    .bind(user_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(StoreError::PermissionDenied)?;
    TeamRole::from_db(&role)
        .ok_or_else(|| StoreError::InvalidData(format!("unknown membership role {role}")))
}

async fn require_team_manager(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: UserId,
    tenant_id: TenantId,
) -> Result<TeamRole, StoreError> {
    let role = require_active_membership(tx, user_id, tenant_id).await?;
    if matches!(role, TeamRole::Owner | TeamRole::Admin) {
        Ok(role)
    } else {
        Err(StoreError::PermissionDenied)
    }
}

async fn bump_policy_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), StoreError> {
    let result = sqlx::query(
        "UPDATE deployments SET policy_revision = policy_revision + 1 WHERE singleton = true",
    )
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::InvalidState(
            "deployment is not initialized".to_owned(),
        ))
    }
}

fn validate_name(value: &str, kind: &str) -> Result<String, StoreError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 128 || value.chars().any(char::is_control) {
        return Err(StoreError::InvalidInput(format!(
            "{kind} name must contain 1 to 128 visible characters"
        )));
    }
    Ok(value.to_owned())
}

fn positive_u32(value: i32) -> Result<u32, StoreError> {
    let value = u32::try_from(value).map_err(invalid_number)?;
    if value == 0 {
        return Err(StoreError::InvalidData(
            "database contains a zero relay rate".to_owned(),
        ));
    }
    Ok(value)
}

fn unix_millis(value: OffsetDateTime) -> Result<i64, StoreError> {
    i64::try_from(value.unix_timestamp_nanos() / 1_000_000).map_err(invalid_number)
}

fn invalid_number(error: impl std::fmt::Display) -> StoreError {
    StoreError::InvalidData(error.to_string())
}

fn map_conflict(error: sqlx::Error, message: &'static str) -> StoreError {
    match &error {
        sqlx::Error::Database(database) if database.code().as_deref() == Some("23505") => {
            StoreError::Conflict(message)
        }
        _ => StoreError::Database(error),
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("resource was not found")]
    NotFound,
    #[error("operation is not permitted in this tenant")]
    PermissionDenied,
    #[error("resource conflict: {0}")]
    Conflict(&'static str),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("stored data violates the application contract: {0}")]
    InvalidData(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Migration(#[from] sqlx::migrate::MigrateError),
}
