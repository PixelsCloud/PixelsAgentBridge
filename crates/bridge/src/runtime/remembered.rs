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
        sqlx::query(
            r#"
            INSERT INTO remembered_devices (
                device_ref_json, device_code, alias, os_family_json, os_reminder
            ) VALUES (?, ?, ?, ?, ?)
            ON CONFLICT (device_ref_json) DO UPDATE SET
                device_code = excluded.device_code,
                os_family_json = excluded.os_family_json,
                os_reminder = excluded.os_reminder
            "#,
        )
        .bind(serde_json::to_string(&device.device_ref)?)
        .bind(device.code.to_string())
        .bind(&device.alias)
        .bind(serde_json::to_string(&device.os_family)?)
        .bind(&device.os_reminder)
        .execute(&self.pool)
        .await?;
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
