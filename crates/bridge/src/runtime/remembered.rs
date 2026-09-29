use pab_protocol::{DeviceCode, DeviceRef, OsFamily};
use sqlx::Row;

use super::store::{RuntimeStore, RuntimeStoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RememberedDevice {
    pub device_ref: DeviceRef,
    pub code: DeviceCode,
    pub alias: String,
    pub os_family: OsFamily,
    pub os_reminder: String,
}

impl RuntimeStore {
    pub async fn remember_device(
        &self,
        device: &RememberedDevice,
    ) -> Result<(), RuntimeStoreError> {
        let device_ref_json = serde_json::to_string(&device.device_ref)?;
        let mut transaction = self.pool.begin().await?;
        let existing_alias: Option<String> =
            sqlx::query_scalar("SELECT alias FROM remembered_devices WHERE device_ref_json = ?")
                .bind(&device_ref_json)
                .fetch_optional(&mut *transaction)
                .await?;
        if existing_alias.is_some() {
            sqlx::query("DELETE FROM remembered_devices WHERE device_ref_json = ?")
                .bind(&device_ref_json)
                .execute(&mut *transaction)
                .await?;
        }
        sqlx::query(
            r#"
            INSERT INTO remembered_devices (
                device_ref_json, device_code, alias, os_family_json, os_reminder
            ) VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(device_ref_json)
        .bind(device.code.to_string())
        .bind(existing_alias.unwrap_or_else(|| device.alias.clone()))
        .bind(serde_json::to_string(&device.os_family)?)
        .bind(&device.os_reminder)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn rename_device(
        &self,
        code: DeviceCode,
        alias: &str,
    ) -> Result<(), RuntimeStoreError> {
        let result = sqlx::query("UPDATE remembered_devices SET alias = ? WHERE device_code = ?")
            .bind(alias)
            .bind(code.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(RuntimeStoreError::NotFound);
        }
        Ok(())
    }

    pub async fn forget_device(
        &self,
        code: DeviceCode,
    ) -> Result<pab_protocol::DeviceId, RuntimeStoreError> {
        let mut transaction = self.pool.begin().await?;
        let stored: Option<String> = sqlx::query_scalar(
            "SELECT device_ref_json FROM remembered_devices WHERE device_code = ?",
        )
        .bind(code.to_string())
        .fetch_optional(&mut *transaction)
        .await?;
        let device_ref: DeviceRef =
            serde_json::from_str(&stored.ok_or(RuntimeStoreError::NotFound)?)?;

        sqlx::query("DELETE FROM remembered_devices WHERE device_code = ?")
            .bind(code.to_string())
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM device_credentials WHERE device_id = ?")
            .bind(device_ref.device_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(device_ref.device_id)
    }

    pub async fn remembered_devices(&self) -> Result<Vec<RememberedDevice>, RuntimeStoreError> {
        let rows = sqlx::query(
            "SELECT device_ref_json, device_code, alias, os_family_json, os_reminder FROM remembered_devices ORDER BY rowid DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let code: String = row.try_get("device_code")?;
                Ok(RememberedDevice {
                    device_ref: serde_json::from_str(row.try_get("device_ref_json")?)?,
                    code: code
                        .parse()
                        .map_err(|_| RuntimeStoreError::InvalidStoredIdentifier)?,
                    alias: row.try_get("alias")?,
                    os_family: serde_json::from_str(row.try_get("os_family_json")?)?,
                    os_reminder: row.try_get("os_reminder")?,
                })
            })
            .collect()
    }
}
