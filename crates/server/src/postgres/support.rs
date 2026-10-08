use super::*;

pub(super) async fn require_account_namespace(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: UserId,
    tenant_id: TenantId,
) -> Result<(), StoreError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users u JOIN personal_tenants p ON p.user_id=u.id JOIN tenants t ON t.id=p.tenant_id WHERE u.id=$1 AND p.tenant_id=$2 AND u.status='active' AND t.status='active')",
    ).bind(user_id.as_uuid()).bind(tenant_id.as_uuid()).fetch_one(&mut **tx).await?;
    if exists {
        Ok(())
    } else {
        Err(StoreError::PermissionDenied)
    }
}

pub(super) async fn bump_policy_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), StoreError> {
    let result = sqlx::query(
        "UPDATE server_settings SET policy_revision = policy_revision + 1 WHERE singleton = true",
    )
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::InvalidState(
            "server settings are not initialized".to_owned(),
        ))
    }
}

pub(super) fn validate_name(value: &str, kind: &str) -> Result<String, StoreError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 128 || value.chars().any(char::is_control) {
        return Err(StoreError::InvalidInput(format!(
            "{kind} name must contain 1 to 128 visible characters"
        )));
    }
    Ok(value.to_owned())
}

pub(super) fn positive_u32(value: i32) -> Result<u32, StoreError> {
    let value = u32::try_from(value).map_err(invalid_number)?;
    if value == 0 {
        return Err(StoreError::InvalidData(
            "database contains a zero relay rate".to_owned(),
        ));
    }
    Ok(value)
}

pub(super) fn unix_millis(value: OffsetDateTime) -> Result<i64, StoreError> {
    i64::try_from(value.unix_timestamp_nanos() / 1_000_000).map_err(invalid_number)
}

pub(super) fn invalid_number(error: impl std::fmt::Display) -> StoreError {
    StoreError::InvalidData(error.to_string())
}

pub(super) fn map_conflict(error: sqlx::Error, message: &'static str) -> StoreError {
    match &error {
        sqlx::Error::Database(database) if database.code().as_deref() == Some("23505") => {
            StoreError::Conflict(message)
        }
        _ => StoreError::Database(error),
    }
}
