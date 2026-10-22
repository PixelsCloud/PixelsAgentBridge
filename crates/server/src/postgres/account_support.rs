use super::{PostgresStore, StoreError};
use serde::Serialize;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

pub(crate) struct SupportActor<'a> {
    pub id: Uuid,
    pub revision: i64,
    pub session_hash: &'a str,
}

pub(crate) struct SupportMutation {
    pub id: Uuid,
    pub request_id: Uuid,
    pub target: Uuid,
    pub kind: &'static str,
    pub scope: Option<&'static str>,
    pub session_id: Option<Uuid>,
    pub revoke_sessions: bool,
    pub revision: Option<i64>,
}

#[derive(Serialize)]
pub(crate) struct SupportOperation {
    pub id: Uuid,
    pub actor_id: Uuid,
    pub target_id: Uuid,
    pub kind: String,
    pub scope: Option<String>,
    pub session_id: Option<Uuid>,
    pub revoke_sessions: bool,
    pub sessions_revoked: i64,
    pub completed_at: i64,
    pub status: &'static str,
    pub propagation: &'static str,
}

impl SupportOperation {
    pub fn matches(&self, actor: Uuid, mutation: &SupportMutation) -> bool {
        self.actor_id == actor
            && self.target_id == mutation.target
            && self.kind == mutation.kind
            && self.scope.as_deref() == mutation.scope
            && self.session_id == mutation.session_id
            && self.revoke_sessions == mutation.revoke_sessions
    }

    fn from_row(row: sqlx::postgres::PgRow) -> Result<Self, StoreError> {
        let completed: OffsetDateTime = row.try_get("completed_at")?;
        let count: i64 = row.try_get("sessions_revoked")?;
        Ok(Self {
            id: row.try_get("id")?,
            actor_id: row.try_get("actor_id")?,
            target_id: row.try_get("target_id")?,
            kind: row.try_get("kind")?,
            scope: row.try_get("scope")?,
            session_id: row.try_get("session_id")?,
            revoke_sessions: row.try_get("revoke_sessions")?,
            sessions_revoked: count,
            completed_at: (completed.unix_timestamp_nanos() / 1_000_000) as i64,
            status: "completed",
            propagation: if count == 0 {
                "not_required"
            } else {
                "pending"
            },
        })
    }
}

impl PostgresStore {
    pub(crate) async fn account_support_operation(
        &self,
        actor: Uuid,
        id: Uuid,
    ) -> Result<Option<SupportOperation>, StoreError> {
        sqlx::query("SELECT * FROM account_support_operations WHERE id=$1 AND actor_id=$2")
            .bind(id)
            .bind(actor)
            .fetch_optional(&self.pool)
            .await?
            .map(SupportOperation::from_row)
            .transpose()
    }

    pub(crate) async fn apply_account_support(
        &self,
        actor: SupportActor<'_>,
        mutation: &SupportMutation,
        password_hash: Option<&str>,
    ) -> Result<SupportOperation, StoreError> {
        let mut tx = self.pool.begin().await?;
        // Same lock as role/status changes: recheck the actor before mutating,
        // including when concurrent administrators revoke one another's access.
        sqlx::query("SELECT pg_advisory_xact_lock(26035001)")
            .execute(&mut *tx)
            .await?;
        let allowed = sqlx::query("SELECT u.server_admin,u.auth_revision FROM users u JOIN web_sessions s ON s.user_id=u.id WHERE u.id=$1 AND s.token_hash=$2 AND u.status='active' FOR UPDATE OF u")
            .bind(actor.id).bind(actor.session_hash).fetch_optional(&mut *tx).await?.ok_or(StoreError::PermissionDenied)?;
        if allowed.try_get::<i64, _>("auth_revision")? != actor.revision
            || (mutation.target != actor.id && !allowed.try_get::<bool, _>("server_admin")?)
        {
            return Err(StoreError::PermissionDenied);
        }
        if let Some(row) = sqlx::query("SELECT * FROM account_support_operations WHERE id=$1")
            .bind(mutation.id)
            .fetch_optional(&mut *tx)
            .await?
        {
            let previous = SupportOperation::from_row(row)?;
            if !previous.matches(actor.id, mutation) {
                return Err(StoreError::Conflict(
                    "operation ID belongs to another action",
                ));
            }
            tx.commit().await?;
            return Ok(previous);
        }
        let target = sqlx::query("SELECT auth_revision,password_hash IS NOT NULL local_password FROM users WHERE id=$1 FOR UPDATE")
            .bind(mutation.target).fetch_optional(&mut *tx).await?.ok_or(StoreError::NotFound)?;
        if mutation.kind == "password_reset" {
            if actor.id == mutation.target {
                return Err(StoreError::InvalidState("use_self_password_change".into()));
            }
            if !target.try_get::<bool, _>("local_password")? {
                return Err(StoreError::InvalidState("password_login_required".into()));
            }
            if Some(target.try_get::<i64, _>("auth_revision")?) != mutation.revision {
                return Err(StoreError::Conflict("account changed"));
            }
            let hash = password_hash
                .ok_or_else(|| StoreError::InvalidInput("password required".into()))?;
            sqlx::query("UPDATE users SET password_hash=$2,auth_revision=auth_revision+1,updated_at=clock_timestamp() WHERE id=$1")
                .bind(mutation.target).bind(hash).execute(&mut *tx).await?;
        }
        let count = if mutation.revoke_sessions {
            let scope = mutation.scope.unwrap_or("all");
            if (scope == "others" && mutation.target != actor.id)
                || ((scope == "session") != mutation.session_id.is_some())
            {
                return Err(StoreError::InvalidInput("invalid session selection".into()));
            }
            sqlx::query("DELETE FROM web_sessions WHERE user_id=$1 AND ($2::text<>'session' OR id=$3) AND ($2::text<>'others' OR token_hash<>$4)")
                .bind(mutation.target).bind(scope).bind(mutation.session_id).bind(actor.session_hash).execute(&mut *tx).await?.rows_affected() as i64
        } else {
            0
        };
        if count > 0 {
            sqlx::query(
                "UPDATE server_settings SET policy_revision=policy_revision+1 WHERE singleton",
            )
            .execute(&mut *tx)
            .await?;
        }
        let row = sqlx::query("INSERT INTO account_support_operations(id,actor_id,target_id,kind,scope,session_id,revoke_sessions,sessions_revoked) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING *")
            .bind(mutation.id).bind(actor.id).bind(mutation.target).bind(mutation.kind).bind(mutation.scope).bind(mutation.session_id).bind(mutation.revoke_sessions).bind(count)
            .fetch_one(&mut *tx).await?;
        // Typed, server-generated fields only. Passwords, hashes and token values
        // are deliberately absent from both the result record and audit.
        let mut record = crate::audit::AuditRecord::new(
            Some(actor.id),
            if mutation.kind == "password_reset" {
                crate::audit::AuditAction::PasswordReset
            } else {
                crate::audit::AuditAction::SessionsRevoked
            },
            crate::audit::AuditResource::Account(mutation.target),
        );
        record.request_id = mutation.request_id;
        record.operation_id = Some(mutation.id);
        record.after = Some(crate::audit::AuditValues {
            password_changed: (mutation.kind == "password_reset").then_some(true),
            sessions_revoked: Some(count),
            ..Default::default()
        });
        record.details = crate::audit::AuditDetails {
            password_changed: Some(mutation.kind == "password_reset"),
            sessions_revoked: Some(count),
            scope: mutation.scope.map(String::from),
            session_id: mutation.session_id,
            ..Default::default()
        };
        self.insert_audit(&mut tx, record).await?;
        tx.commit().await?;
        SupportOperation::from_row(row)
    }
}
