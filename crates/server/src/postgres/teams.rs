use super::*;

impl PostgresStore {
    pub async fn create_team(&self, actor: UserId, name: &str) -> Result<Team, StoreError> {
        let name = support::validate_name(name, "Team")?;
        let tenant_id = TenantId::new();
        let mut tx = self.pool.begin().await?;
        support::require_active_user(&mut tx, actor).await?;

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
        support::bump_policy_revision(&mut tx).await?;
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
        let actor_role = support::require_team_manager(&mut tx, actor, tenant_id).await?;
        if actor_role == TeamRole::Admin && role != TeamRole::Member {
            return Err(StoreError::PermissionDenied);
        }
        support::require_active_user(&mut tx, invited_user).await?;

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
            return Err(support::map_conflict(
                error,
                "a pending invitation already exists",
            ));
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
        support::bump_policy_revision(&mut tx).await?;
        tx.commit().await?;
        Ok(tenant_id)
    }
}
