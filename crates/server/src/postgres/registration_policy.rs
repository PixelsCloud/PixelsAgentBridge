use super::PostgresStore;
use serde::Serialize;
use sqlx::Row;

#[derive(Debug, Clone, Serialize)]
pub struct RegistrationPolicy {
    pub deployment_default: bool,
    pub override_value: Option<bool>,
    pub effective: bool,
    pub source: &'static str,
    pub revision: i64,
    pub effect: &'static str,
}
impl RegistrationPolicy {
    pub(crate) fn resolve(default: bool, value: Option<bool>, revision: i64) -> Self {
        Self {
            deployment_default: default,
            override_value: value,
            effective: value.unwrap_or(default),
            source: if value.is_some() {
                "runtime_override"
            } else {
                "deployment_default"
            },
            revision,
            effect: "immediate",
        }
    }
}
impl PostgresStore {
    pub async fn registration_policy(
        &self,
        default: bool,
    ) -> Result<RegistrationPolicy, sqlx::Error> {
        let row=sqlx::query("SELECT registration_override,registration_revision FROM server_settings WHERE singleton")
            .fetch_one(&self.pool).await?;
        Ok(RegistrationPolicy::resolve(
            default,
            row.try_get("registration_override")?,
            row.try_get("registration_revision")?,
        ))
    }

    /// Hold the policy row through account creation. A policy update waits for
    /// earlier admitted registrations; once it commits, new admissions see it.
    pub(crate) async fn require_registration(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        default: bool,
    ) -> Result<(), super::StoreError> {
        let value: Option<bool> = sqlx::query_scalar(
            "SELECT registration_override FROM server_settings WHERE singleton FOR SHARE",
        )
        .fetch_one(&mut **tx)
        .await?;
        if !value.unwrap_or(default) {
            return Err(super::StoreError::InvalidState(
                "registration_disabled".into(),
            ));
        }
        Ok(())
    }
}
