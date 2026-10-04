use super::*;

impl PostgresStore {
    pub async fn list_traffic_scopes(
        &self,
        user_id: UserId,
        personal_tenant_id: TenantId,
    ) -> Result<pab_protocol::TrafficScopeOptions, StoreError> {
        let account_scope = sqlx::query(
            "SELECT settings.default_personal_mbps, account.default_traffic_team_id \
             FROM server_settings settings CROSS JOIN users account \
             WHERE settings.singleton = true AND account.id = $1",
        )
        .bind(user_id.as_uuid())
        .fetch_one(&self.pool)
        .await?;
        let personal_mbps: i32 = account_scope.try_get("default_personal_mbps")?;
        let assigned_team: Option<Uuid> = account_scope.try_get("default_traffic_team_id")?;
        let rows = sqlx::query(
            r#"
            SELECT team.tenant_id, team.name,
                   COALESCE(team.relay_total_mbps, settings.default_team_mbps) AS total_mbps,
                   COALESCE(team.relay_member_mbps, settings.default_member_mbps) AS member_mbps
            FROM memberships member
            JOIN teams team ON team.tenant_id = member.tenant_id
            JOIN tenants tenant ON tenant.id = team.tenant_id AND tenant.status = 'active'
            JOIN users account ON account.id = member.user_id AND account.status = 'active'
            JOIN server_settings settings ON settings.singleton = true
            WHERE member.user_id = $1 AND member.status = 'active'
            ORDER BY team.name, team.tenant_id
            "#,
        )
        .bind(user_id.as_uuid())
        .fetch_all(&self.pool)
        .await?;
        let teams = rows
            .into_iter()
            .map(|row| {
                Ok(pab_protocol::TeamTrafficScope {
                    tenant_id: TenantId::from_uuid(row.try_get("tenant_id")?),
                    name: row.try_get("name")?,
                    total_mbps: u32::try_from(row.try_get::<i32, _>("total_mbps")?)
                        .map_err(support::invalid_number)?,
                    member_mbps: u32::try_from(row.try_get::<i32, _>("member_mbps")?)
                        .map_err(support::invalid_number)?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        let default_tenant_id = assigned_team
            .map(TenantId::from_uuid)
            .filter(|tenant_id| {
                teams
                    .iter()
                    .any(|team: &pab_protocol::TeamTrafficScope| team.tenant_id == *tenant_id)
            })
            .unwrap_or(personal_tenant_id);
        Ok(pab_protocol::TrafficScopeOptions {
            personal_tenant_id,
            default_tenant_id,
            personal_mbps: u32::try_from(personal_mbps).map_err(support::invalid_number)?,
            teams,
        })
    }

    pub async fn admin_create_team(
        &self,
        owner: UserId,
        name: &str,
        operator_label: &str,
    ) -> Result<Team, StoreError> {
        let name = support::validate_name(name, "Team")?;
        let operator_label = support::validate_name(operator_label, "administrator")?;
        let tenant_id = TenantId::new();
        let mut tx = self.pool.begin().await?;
        support::require_active_user(&mut tx, owner).await?;

        sqlx::query("INSERT INTO tenants (id, kind, created_by_user_id) VALUES ($1, 'team', $2)")
            .bind(tenant_id.as_uuid())
            .bind(owner.as_uuid())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO teams (tenant_id, name) VALUES ($1, $2)")
            .bind(tenant_id.as_uuid())
            .bind(&name)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO memberships (tenant_id, user_id, role) VALUES ($1, $2, 'owner')")
            .bind(tenant_id.as_uuid())
            .bind(owner.as_uuid())
            .execute(&mut *tx)
            .await?;
        record_team_admin_event(
            &mut tx,
            tenant_id,
            owner,
            "team_created",
            TeamRole::Owner,
            &operator_label,
        )
        .await?;
        support::bump_policy_revision(&mut tx).await?;
        tx.commit().await?;

        Ok(Team { tenant_id, name })
    }

    pub async fn admin_add_team_member(
        &self,
        tenant_id: TenantId,
        user_id: UserId,
        role: TeamRole,
        operator_label: &str,
    ) -> Result<bool, StoreError> {
        if role == TeamRole::Owner {
            return Err(StoreError::InvalidInput(
                "owner transfer is a separate operation".to_owned(),
            ));
        }
        let operator_label = support::validate_name(operator_label, "administrator")?;
        let mut tx = self.pool.begin().await?;
        support::require_active_user(&mut tx, user_id).await?;
        let team_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM teams JOIN tenants ON tenants.id = teams.tenant_id WHERE teams.tenant_id = $1 AND tenants.status = 'active')",
        )
        .bind(tenant_id.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
        if !team_exists {
            return Err(StoreError::NotFound);
        }

        let inserted = sqlx::query(
            "INSERT INTO memberships (tenant_id, user_id, role) VALUES ($1, $2, $3) ON CONFLICT (tenant_id, user_id) DO NOTHING",
        )
        .bind(tenant_id.as_uuid())
        .bind(user_id.as_uuid())
        .bind(role.as_db())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if !inserted {
            let existing = sqlx::query(
                "SELECT role, status FROM memberships WHERE tenant_id = $1 AND user_id = $2 FOR UPDATE",
            )
            .bind(tenant_id.as_uuid())
            .bind(user_id.as_uuid())
            .fetch_one(&mut *tx)
            .await?;
            let existing_role: String = existing.try_get("role")?;
            let status: String = existing.try_get("status")?;
            if status == "active" && existing_role == role.as_db() {
                return Ok(false);
            }
            if status != "removed" || existing_role == "owner" {
                return Err(StoreError::Conflict(
                    "active role changes require a separate operation",
                ));
            }
            sqlx::query(
                "UPDATE memberships SET role = $3, status = 'active', revision = revision + 1, updated_at = now() WHERE tenant_id = $1 AND user_id = $2",
            )
            .bind(tenant_id.as_uuid())
            .bind(user_id.as_uuid())
            .bind(role.as_db())
            .execute(&mut *tx)
            .await?;
        }

        record_team_admin_event(
            &mut tx,
            tenant_id,
            user_id,
            "member_added",
            role,
            &operator_label,
        )
        .await?;
        support::bump_policy_revision(&mut tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn admin_remove_team_member(
        &self,
        tenant_id: TenantId,
        user_id: UserId,
        operator_label: &str,
    ) -> Result<bool, StoreError> {
        let operator_label = support::validate_name(operator_label, "administrator")?;
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            r#"
            SELECT member.role, member.status
            FROM teams team
            JOIN tenants tenant ON tenant.id = team.tenant_id AND tenant.status = 'active'
            LEFT JOIN memberships member
              ON member.tenant_id = team.tenant_id AND member.user_id = $2
            WHERE team.tenant_id = $1
            FOR UPDATE OF team
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(user_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::NotFound)?;
        let role: Option<String> = row.try_get("role")?;
        let status: Option<String> = row.try_get("status")?;
        if role.as_deref() == Some("owner") {
            return Err(StoreError::InvalidInput(
                "owner transfer is a separate operation".to_owned(),
            ));
        }
        if status.as_deref() != Some("active") {
            return Ok(false);
        }
        let role = TeamRole::from_db(role.as_deref().unwrap_or_default())
            .ok_or_else(|| StoreError::InvalidData("unknown Team role".to_owned()))?;
        sqlx::query(
            "UPDATE memberships SET status = 'removed', revision = revision + 1, updated_at = now() WHERE tenant_id = $1 AND user_id = $2",
        )
        .bind(tenant_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&mut *tx)
        .await?;
        let cleared_default = sqlx::query(
            "UPDATE users SET default_traffic_team_id = NULL, updated_at = now() \
             WHERE id = $1 AND default_traffic_team_id = $2",
        )
        .bind(user_id.as_uuid())
        .bind(tenant_id.as_uuid())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if cleared_default {
            record_traffic_assignment_event(
                &mut tx,
                user_id,
                Some(tenant_id),
                None,
                &operator_label,
            )
            .await?;
        }
        record_team_admin_event(
            &mut tx,
            tenant_id,
            user_id,
            "member_removed",
            role,
            &operator_label,
        )
        .await?;
        support::bump_policy_revision(&mut tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn admin_set_default_traffic_team(
        &self,
        user_id: UserId,
        team_id: Option<TenantId>,
        operator_label: &str,
    ) -> Result<bool, StoreError> {
        let operator_label = support::validate_name(operator_label, "administrator")?;
        let mut tx = self.pool.begin().await?;
        let old_team: Option<Uuid> = sqlx::query_scalar(
            "SELECT default_traffic_team_id FROM users WHERE id = $1 AND status = 'active' FOR UPDATE",
        )
        .bind(user_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::NotFound)?;
        if let Some(team_id) = team_id {
            let valid: bool = sqlx::query_scalar(
                "SELECT EXISTS ( \
                 SELECT 1 FROM memberships member \
                 JOIN teams team ON team.tenant_id = member.tenant_id \
                 JOIN tenants tenant ON tenant.id = team.tenant_id AND tenant.status = 'active' \
                 WHERE member.user_id = $1 AND member.tenant_id = $2 AND member.status = 'active')",
            )
            .bind(user_id.as_uuid())
            .bind(team_id.as_uuid())
            .fetch_one(&mut *tx)
            .await?;
            if !valid {
                return Err(StoreError::InvalidInput(
                    "account is not an active member of the selected Team".to_owned(),
                ));
            }
        }
        if old_team == team_id.map(|id| id.as_uuid()) {
            return Ok(false);
        }
        sqlx::query(
            "UPDATE users SET default_traffic_team_id = $2, updated_at = now() WHERE id = $1",
        )
        .bind(user_id.as_uuid())
        .bind(team_id.map(|id| id.as_uuid()))
        .execute(&mut *tx)
        .await?;
        record_traffic_assignment_event(
            &mut tx,
            user_id,
            old_team.map(TenantId::from_uuid),
            team_id,
            &operator_label,
        )
        .await?;
        support::bump_policy_revision(&mut tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn admin_set_team_limits(
        &self,
        tenant_id: TenantId,
        total_mbps: u32,
        member_mbps: u32,
        operator_label: &str,
    ) -> Result<bool, StoreError> {
        let operator_label = support::validate_name(operator_label, "administrator")?;
        let total_mbps = i32::try_from(total_mbps)
            .map_err(|_| StoreError::InvalidInput("Team speed is too large".to_owned()))?;
        let member_mbps = i32::try_from(member_mbps)
            .map_err(|_| StoreError::InvalidInput("member speed is too large".to_owned()))?;
        if member_mbps <= 0 || total_mbps < member_mbps {
            return Err(StoreError::InvalidInput(
                "speeds must be positive and member speed must not exceed Team speed".to_owned(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            r#"
            SELECT COALESCE(team.relay_total_mbps, settings.default_team_mbps) AS total_mbps,
                   COALESCE(team.relay_member_mbps, settings.default_member_mbps) AS member_mbps,
                   owner.user_id AS owner_user_id
            FROM teams team
            JOIN tenants tenant ON tenant.id = team.tenant_id AND tenant.status = 'active'
            JOIN memberships owner
              ON owner.tenant_id = team.tenant_id
             AND owner.role = 'owner' AND owner.status = 'active'
            JOIN server_settings settings ON settings.singleton = true
            WHERE team.tenant_id = $1
            FOR UPDATE OF team
            "#,
        )
        .bind(tenant_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::NotFound)?;
        let old_total: i32 = row.try_get("total_mbps")?;
        let old_member: i32 = row.try_get("member_mbps")?;
        if old_total == total_mbps && old_member == member_mbps {
            return Ok(false);
        }
        let owner_id: Uuid = row.try_get("owner_user_id")?;
        sqlx::query(
            "UPDATE teams SET relay_total_mbps = $2, relay_member_mbps = $3 WHERE tenant_id = $1",
        )
        .bind(tenant_id.as_uuid())
        .bind(total_mbps)
        .bind(member_mbps)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO team_admin_events
                (id, tenant_id, target_user_id, action, role, operator_label,
                 old_total_mbps, old_member_mbps, new_total_mbps, new_member_mbps)
            VALUES ($1, $2, $3, 'limits_changed', 'owner', $4, $5, $6, $7, $8)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id.as_uuid())
        .bind(owner_id)
        .bind(&operator_label)
        .bind(old_total)
        .bind(old_member)
        .bind(total_mbps)
        .bind(member_mbps)
        .execute(&mut *tx)
        .await?;
        support::bump_policy_revision(&mut tx).await?;
        tx.commit().await?;
        Ok(true)
    }
}

async fn record_team_admin_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: UserId,
    action: &str,
    role: TeamRole,
    operator_label: &str,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO team_admin_events (id, tenant_id, target_user_id, action, role, operator_label) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(tenant_id.as_uuid())
    .bind(user_id.as_uuid())
    .bind(action)
    .bind(role.as_db())
    .bind(operator_label)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn record_traffic_assignment_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: UserId,
    old_team: Option<TenantId>,
    new_team: Option<TenantId>,
    operator_label: &str,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO account_traffic_assignment_events \
         (id, user_id, old_team_id, new_team_id, operator_label) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(user_id.as_uuid())
    .bind(old_team.map(|id| id.as_uuid()))
    .bind(new_team.map(|id| id.as_uuid()))
    .bind(operator_label)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
