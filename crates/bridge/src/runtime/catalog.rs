use super::{RememberedDevice, RuntimeStore, RuntimeStoreError};
use pab_agent_core::account::{AccountClient, AccountError, AccountStore};
use pab_protocol::{DeviceCode, DeviceId, OsFamily, SavedDevice, SavedDeviceMutation};
use sqlx::Row;

pub(super) fn scope(store: &RuntimeStore) -> Result<Option<String>, RuntimeStoreError> {
    scope_at(store, None)
}

fn scope_at(
    store: &RuntimeStore,
    expected: Option<u64>,
) -> Result<Option<String>, RuntimeStoreError> {
    let Some(origin) = &store.account_origin else {
        return Ok(None);
    };
    let paths = pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::User)
        .map_err(|_| RuntimeStoreError::AccountStorage)?;
    let state = AccountStore::new(paths.root(), origin)
        .and_then(|v| v.read())
        .map_err(|_| RuntimeStoreError::AccountStorage)?;
    if expected.is_some_and(|revision| revision != state.revision) {
        return Err(RuntimeStoreError::AccountStorage);
    }
    Ok(state.user.map(|u| format!("{origin}\n{}", u.id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn account_catalog_edits_are_partitioned_and_deleted_entries_stay_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.db");
        let store = RuntimeStore::open(&path).await.unwrap();
        let device = RememberedDevice {
            device_ref: pab_protocol::DeviceRef {
                device_id: DeviceId::new(),
                tenant_id: pab_protocol::TenantId::new(),
            },
            code: "123456789".parse().unwrap(),
            alias: "machine".into(),
            os_family: Some(OsFamily::Windows),
            os_reminder: String::new(),
        };
        store
            .catalog_remember("https://one\nA", &device)
            .await
            .unwrap();
        store
            .catalog_remember("https://one\nB", &device)
            .await
            .unwrap();
        store
            .catalog_edit("https://one\nA", device.code, Some("private alias"), None)
            .await
            .unwrap();
        assert_eq!(
            store.catalog_list("https://one\nB").await.unwrap()[0].alias,
            "machine"
        );
        assert!(
            store
                .catalog_list("https://two\nA")
                .await
                .unwrap()
                .is_empty()
        );
        store
            .catalog_edit("https://one\nA", device.code, None, Some(true))
            .await
            .unwrap();
        assert!(
            store
                .catalog_list("https://one\nA")
                .await
                .unwrap()
                .is_empty()
        );
        store.close().await;
        let store = RuntimeStore::open(&path).await.unwrap();
        assert!(
            store
                .catalog_list("https://one\nA")
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.catalog_list("https://one\nB").await.unwrap().len(), 1);
        sqlx::query("INSERT INTO catalog_outbox(scope,device_id,mutation_json) VALUES('https://one'||char(10)||'B',?,'{}')").bind(device.device_ref.device_id.to_string()).execute(&store.pool).await.unwrap();
        assert!(matches!(
            store
                .catalog_edit("https://one\nB", device.code, Some("overwrite"), None)
                .await,
            Err(RuntimeStoreError::CatalogPending)
        ));
        store.close().await;
    }
}

fn remembered(item: SavedDevice) -> Result<RememberedDevice, RuntimeStoreError> {
    Ok(RememberedDevice {
        device_ref: item.device_ref,
        code: item
            .code
            .parse()
            .map_err(|_| RuntimeStoreError::InvalidStoredIdentifier)?,
        alias: if item.alias.is_empty() {
            item.name
        } else {
            item.alias
        },
        os_family: match item.system.as_deref() {
            Some("windows") => Some(OsFamily::Windows),
            Some("macos") => Some(OsFamily::Macos),
            Some("linux") => Some(OsFamily::Linux),
            _ => None,
        },
        os_reminder: item.system.unwrap_or_default(),
    })
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct CatalogStatus {
    pub pending: i64,
    pub syncing: bool,
    pub error: Option<String>,
    pub conflicts: Vec<SavedDevice>,
    pub local_import_count: i64,
}

impl RuntimeStore {
    pub(super) async fn catalog_status(&self) -> Result<CatalogStatus, RuntimeStoreError> {
        let scope = scope(self)?.ok_or(RuntimeStoreError::AccountStorage)?;
        let pending:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM account_catalog WHERE scope=? AND dirty=1)+(SELECT count(*) FROM catalog_imports WHERE scope=?)").bind(&scope).bind(&scope).fetch_one(&self.pool).await?;
        let sync = sqlx::query("SELECT error,lease,lease_until FROM catalog_sync WHERE scope=?")
            .bind(&scope)
            .fetch_optional(&self.pool)
            .await?;
        let conflicts: Vec<String> = sqlx::query_scalar(
            "SELECT desired_json FROM catalog_conflicts WHERE scope=? LIMIT 100",
        )
        .bind(&scope)
        .fetch_all(&self.pool)
        .await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM remembered_devices")
            .fetch_one(&self.pool)
            .await?;
        Ok(CatalogStatus {
            pending,
            syncing: sync.as_ref().is_some_and(|r| {
                r.get::<Option<String>, _>("lease").is_some()
                    && r.get::<i64, _>("lease_until") > super::unix_millis()
            }),
            error: sync.and_then(|r| r.get("error")),
            conflicts: conflicts
                .into_iter()
                .map(|v| serde_json::from_str(&v))
                .collect::<Result<_, _>>()?,
            local_import_count: count,
        })
    }
    pub(super) async fn catalog_import_local(
        &self,
        expected: Option<u64>,
    ) -> Result<usize, RuntimeStoreError> {
        let scope = scope_at(self, expected)?.ok_or(RuntimeStoreError::AccountStorage)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let rows = sqlx::query("SELECT * FROM remembered_devices")
            .fetch_all(&mut *tx)
            .await?;
        let mut count = 0;
        for row in rows {
            let device_ref: pab_protocol::DeviceRef =
                serde_json::from_str(row.get("device_ref_json"))?;
            let os: Option<OsFamily> = serde_json::from_str(row.get("os_family_json"))?;
            let alias: String = row.get("alias");
            let input = pab_protocol::SavedDeviceImport {
                device_ref,
                code: row.get("device_code"),
                name: alias.clone(),
                alias,
                system: os.map(|os| {
                    match os {
                        OsFamily::Windows => "windows",
                        OsFamily::Macos => "macos",
                        OsFamily::Linux => "linux",
                    }
                    .into()
                }),
            };
            count += sqlx::query(
                "INSERT OR IGNORE INTO catalog_imports(scope,device_id,payload) VALUES(?,?,?)",
            )
            .bind(&scope)
            .bind(device_ref.device_id.to_string())
            .bind(serde_json::to_string(&input)?)
            .execute(&mut *tx)
            .await?
            .rows_affected() as usize;
        }
        tx.commit().await?;
        Ok(count)
    }
    pub(super) async fn catalog_resolve(
        &self,
        device: DeviceId,
        keep_local: bool,
        expected: Option<u64>,
    ) -> Result<(), RuntimeStoreError> {
        let scope = scope_at(self, expected)?.ok_or(RuntimeStoreError::AccountStorage)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let conflict: String = sqlx::query_scalar(
            "SELECT desired_json FROM catalog_conflicts WHERE scope=? AND device_id=?",
        )
        .bind(&scope)
        .bind(device.to_string())
        .fetch_one(&mut *tx)
        .await?;
        let inflight: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM catalog_outbox WHERE scope=? AND device_id=?)",
        )
        .bind(&scope)
        .bind(device.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if inflight {
            return Err(RuntimeStoreError::CatalogPending);
        }
        if keep_local {
            let desired: SavedDevice = serde_json::from_str(&conflict)?;
            let current: String = sqlx::query_scalar(
                "SELECT item_json FROM account_catalog WHERE scope=? AND device_id=?",
            )
            .bind(&scope)
            .bind(device.to_string())
            .fetch_one(&mut *tx)
            .await?;
            let mut item: SavedDevice = serde_json::from_str(&current)?;
            item.alias = desired.alias;
            item.deleted = desired.deleted;
            sqlx::query(
                "UPDATE account_catalog SET item_json=?,dirty=1 WHERE scope=? AND device_id=?",
            )
            .bind(serde_json::to_string(&item)?)
            .bind(&scope)
            .bind(device.to_string())
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("DELETE FROM catalog_conflicts WHERE scope=? AND device_id=?")
            .bind(&scope)
            .bind(device.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub(super) async fn catalog_list(
        &self,
        scope: &str,
    ) -> Result<Vec<RememberedDevice>, RuntimeStoreError> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT item_json FROM account_catalog WHERE scope=? ORDER BY rowid DESC",
        )
        .bind(scope)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|s| serde_json::from_str::<SavedDevice>(&s).map_err(Into::into))
            .collect::<Result<Vec<_>, RuntimeStoreError>>()?
            .into_iter()
            .filter(|v| !v.deleted)
            .map(remembered)
            .collect()
    }

    pub(super) async fn catalog_remember(
        &self,
        scope: &str,
        device: &RememberedDevice,
    ) -> Result<(), RuntimeStoreError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let old: Option<String> = sqlx::query_scalar(
            "SELECT item_json FROM account_catalog WHERE scope=? AND device_id=?",
        )
        .bind(scope)
        .bind(device.device_ref.device_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(old) = old {
            let mut item: SavedDevice = serde_json::from_str(&old)?;
            if let Some(os) = device.os_family {
                item.system = Some(
                    match os {
                        OsFamily::Windows => "windows",
                        OsFamily::Macos => "macos",
                        OsFamily::Linux => "linux",
                    }
                    .into(),
                );
                sqlx::query("UPDATE account_catalog SET item_json=? WHERE scope=? AND device_id=?")
                    .bind(serde_json::to_string(&item)?)
                    .bind(scope)
                    .bind(device.device_ref.device_id.to_string())
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
            if item.deleted {
                self.catalog_edit(scope, device.code, None, Some(false))
                    .await?;
            }
            return Ok(());
        }
        let item = SavedDevice {
            device_ref: device.device_ref,
            code: device.code.to_string(),
            name: device.alias.clone(),
            system: device.os_family.map(|os| {
                match os {
                    OsFamily::Windows => "windows",
                    OsFamily::Macos => "macos",
                    OsFamily::Linux => "linux",
                }
                .into()
            }),
            alias: device.alias.clone(),
            revision: 0,
            deleted: false,
            online: None,
        };
        sqlx::query("INSERT INTO account_catalog(scope,device_id,code,item_json,revision,dirty) VALUES(?,?,?,?,0,0)")
            .bind(scope).bind(device.device_ref.device_id.to_string()).bind(device.code.to_string()).bind(serde_json::to_string(&item)?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub(super) async fn catalog_edit(
        &self,
        scope: &str,
        code: DeviceCode,
        alias: Option<&str>,
        deleted: Option<bool>,
    ) -> Result<DeviceId, RuntimeStoreError> {
        if alias.is_some_and(|v| v.chars().count() > 128 || v.chars().any(char::is_control)) {
            return Err(RuntimeStoreError::InvalidStoredIdentifier);
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row = sqlx::query("SELECT item_json FROM account_catalog WHERE scope=? AND code=?")
            .bind(scope)
            .bind(code.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(RuntimeStoreError::NotFound)?;
        let mut item: SavedDevice = serde_json::from_str(row.try_get("item_json")?)?;
        let inflight: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM catalog_outbox WHERE scope=? AND device_id=?)",
        )
        .bind(scope)
        .bind(item.device_ref.device_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if inflight {
            return Err(RuntimeStoreError::CatalogPending);
        }
        if let Some(alias) = alias {
            item.alias = alias.to_owned();
        }
        if let Some(deleted) = deleted {
            item.deleted = deleted;
        }
        sqlx::query("UPDATE account_catalog SET item_json=?,dirty=1 WHERE scope=? AND device_id=?")
            .bind(serde_json::to_string(&item)?)
            .bind(scope)
            .bind(item.device_ref.device_id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(item.device_ref.device_id)
    }

    /// The lease is in SQLite so independent MCP processes share one synchronizer per account.
    pub(super) async fn sync_catalog(
        &self,
        client: &AccountClient,
        account_store: &AccountStore,
    ) -> Result<(), AccountError> {
        let state = account_store.read()?;
        let Some(user) = state.user else {
            return Ok(());
        };
        let token = account_store
            .token(state.active_slot.as_deref().ok_or(AccountError::Storage)?)?
            .ok_or(AccountError::Storage)?;
        let scope = format!("{}\n{}", client.origin(), user.id);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| AccountError::Storage)?
            .as_millis() as i64;
        let lease = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT OR IGNORE INTO catalog_sync(scope) VALUES(?)")
            .bind(&scope)
            .execute(&self.pool)
            .await
            .map_err(|_| AccountError::Storage)?;
        let acquired = sqlx::query(
            "UPDATE catalog_sync SET lease=?,lease_until=? WHERE scope=? AND lease_until<?",
        )
        .bind(&lease)
        .bind(now + 120_000)
        .bind(&scope)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|_| AccountError::Storage)?;
        if acquired.rows_affected() == 0 {
            return Ok(());
        }
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            self.sync_catalog_claimed(client, account_store, state.revision, &token, &scope),
        )
        .await
        .unwrap_or(Err(AccountError::Network));
        let failures: i64 = sqlx::query_scalar("SELECT failures FROM catalog_sync WHERE scope=?")
            .bind(&scope)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AccountError::Storage)?;
        let next_failures = if result.is_err() {
            (failures + 1).min(7)
        } else {
            0
        };
        let jitter = (uuid::Uuid::new_v4().as_u128() % 2000) as i64;
        let delay = if result.is_err() {
            (5_000_i64 << next_failures).min(300_000) + jitter
        } else {
            10_000
        };
        sqlx::query("UPDATE catalog_sync SET lease=NULL,lease_until=?,error=?,failures=? WHERE scope=? AND lease=?")
            .bind(super::unix_millis() + delay)
            .bind(result.as_ref().err().map(|_| "device list sync pending"))
            .bind(next_failures)
            .bind(&scope)
            .bind(lease)
            .execute(&self.pool)
            .await
            .map_err(|_| AccountError::Storage)?;
        result
    }

    async fn sync_catalog_claimed(
        &self,
        client: &AccountClient,
        store: &AccountStore,
        account_revision: u64,
        token: &str,
        scope: &str,
    ) -> Result<(), AccountError> {
        let imports: Vec<String> =
            sqlx::query_scalar("SELECT payload FROM catalog_imports WHERE scope=? LIMIT 20")
                .bind(scope)
                .fetch_all(&self.pool)
                .await
                .map_err(|_| AccountError::Storage)?;
        for value in imports {
            if store.read()?.revision != account_revision {
                return Ok(());
            }
            let import: pab_protocol::SavedDeviceImport =
                serde_json::from_str(&value).map_err(|_| AccountError::Storage)?;
            match client.import_saved_device(token, &import).await {
                Ok(()) => {}
                Err(AccountError::Http(404)) => {}
                Err(error) => return Err(error),
            }
            sqlx::query("DELETE FROM catalog_imports WHERE scope=? AND device_id=?")
                .bind(scope)
                .bind(import.device_ref.device_id.to_string())
                .execute(&self.pool)
                .await
                .map_err(|_| AccountError::Storage)?;
        }
        // Bound every pass; the next lease continues where this pass stopped.
        let mut cursor: i64 = sqlx::query_scalar("SELECT cursor FROM catalog_sync WHERE scope=?")
            .bind(scope)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AccountError::Storage)?;
        for _ in 0..4 {
            if store.read()?.revision != account_revision {
                return Ok(());
            }
            let changes = client.saved_devices(token, cursor).await?;
            let mut tx = self
                .pool
                .begin_with("BEGIN IMMEDIATE")
                .await
                .map_err(|_| AccountError::Storage)?;
            for remote in &changes.items {
                let row=sqlx::query("SELECT revision,dirty,item_json FROM account_catalog WHERE scope=? AND device_id=?")
                    .bind(scope).bind(remote.device_ref.device_id.to_string()).fetch_optional(&mut *tx).await.map_err(|_|AccountError::Storage)?;
                let mut item = remote.clone();
                let dirty = row.as_ref().is_some_and(|r| r.get::<i64, _>("dirty") != 0);
                if let Some(row) = &row {
                    let local_revision: i64 = row.get("revision");
                    if local_revision > remote.revision {
                        continue;
                    }
                    if dirty {
                        let local: SavedDevice = serde_json::from_str(row.get("item_json"))
                            .map_err(|_| AccountError::Storage)?;
                        // First observation can supply a revision for a queued edit; later conflicts must not overwrite the remote change.
                        if local_revision == 0 || local_revision == remote.revision {
                            item.alias = local.alias;
                            item.deleted = local.deleted;
                        } else if local.alias != remote.alias || local.deleted != remote.deleted {
                            sqlx::query("INSERT INTO catalog_conflicts(scope,device_id,desired_json) VALUES(?,?,?) ON CONFLICT(scope,device_id) DO NOTHING").bind(scope).bind(remote.device_ref.device_id.to_string()).bind(serde_json::to_string(&local).map_err(|_|AccountError::Storage)?).execute(&mut *tx).await.map_err(|_|AccountError::Storage)?;
                        }
                    }
                }
                let keep_dirty = dirty
                    && row.as_ref().is_some_and(|r| {
                        let v = r.get::<i64, _>("revision");
                        v == 0 || v == remote.revision
                    });
                sqlx::query("INSERT INTO account_catalog(scope,device_id,code,item_json,revision,dirty) VALUES(?,?,?,?,?,?) ON CONFLICT(scope,device_id) DO UPDATE SET code=excluded.code,item_json=excluded.item_json,revision=excluded.revision,dirty=excluded.dirty")
                    .bind(scope).bind(item.device_ref.device_id.to_string()).bind(&item.code).bind(serde_json::to_string(&item).map_err(|_|AccountError::Storage)?)
                    .bind(remote.revision).bind(keep_dirty as i64).execute(&mut *tx).await.map_err(|_|AccountError::Storage)?;
            }
            cursor = changes.cursor;
            sqlx::query("UPDATE catalog_sync SET cursor=? WHERE scope=?")
                .bind(cursor)
                .bind(scope)
                .execute(&mut *tx)
                .await
                .map_err(|_| AccountError::Storage)?;
            tx.commit().await.map_err(|_| AccountError::Storage)?;
            if !changes.has_more {
                break;
            }
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|_| AccountError::Storage)?;
        let rows:Vec<String>=sqlx::query_scalar("SELECT item_json FROM account_catalog WHERE scope=? AND dirty=1 AND revision>0 AND NOT EXISTS(SELECT 1 FROM catalog_outbox o WHERE o.scope=account_catalog.scope AND o.device_id=account_catalog.device_id) LIMIT 20")
            .bind(scope).fetch_all(&mut *tx).await.map_err(|_|AccountError::Storage)?;
        for value in rows {
            let item: SavedDevice =
                serde_json::from_str(&value).map_err(|_| AccountError::Storage)?;
            let mutation = SavedDeviceMutation {
                id: uuid::Uuid::new_v4(),
                device_id: item.device_ref.device_id,
                expected_revision: item.revision,
                alias: item.alias,
                deleted: item.deleted,
            };
            sqlx::query("INSERT INTO catalog_outbox(scope,device_id,mutation_json) VALUES(?,?,?)")
                .bind(scope)
                .bind(mutation.device_id.to_string())
                .bind(serde_json::to_string(&mutation).map_err(|_| AccountError::Storage)?)
                .execute(&mut *tx)
                .await
                .map_err(|_| AccountError::Storage)?;
        }
        tx.commit().await.map_err(|_| AccountError::Storage)?;
        let pending: Vec<String> =
            sqlx::query_scalar("SELECT mutation_json FROM catalog_outbox WHERE scope=? LIMIT 20")
                .bind(scope)
                .fetch_all(&self.pool)
                .await
                .map_err(|_| AccountError::Storage)?;
        for value in pending {
            if store.read()?.revision != account_revision {
                return Ok(());
            }
            let mutation: SavedDeviceMutation =
                serde_json::from_str(&value).map_err(|_| AccountError::Storage)?;
            let result = client.update_saved_device(token, &mutation).await;
            let revision = match result {
                Ok(revision) => Some(revision),
                Err(AccountError::Http(409)) => None,
                Err(error) => return Err(error),
            };
            let mut tx = self
                .pool
                .begin_with("BEGIN IMMEDIATE")
                .await
                .map_err(|_| AccountError::Storage)?;
            if let Some(revision) = revision {
                let mut item: SavedDevice = serde_json::from_str(
                    &sqlx::query_scalar::<_, String>(
                        "SELECT item_json FROM account_catalog WHERE scope=? AND device_id=?",
                    )
                    .bind(scope)
                    .bind(mutation.device_id.to_string())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|_| AccountError::Storage)?,
                )
                .map_err(|_| AccountError::Storage)?;
                if item.revision <= revision {
                    item.revision = revision;
                    item.alias = mutation.alias;
                    item.deleted = mutation.deleted;
                    sqlx::query("UPDATE account_catalog SET item_json=?,revision=?,dirty=0 WHERE scope=? AND device_id=?")
                        .bind(serde_json::to_string(&item).map_err(|_|AccountError::Storage)?).bind(revision).bind(scope).bind(mutation.device_id.to_string()).execute(&mut *tx).await.map_err(|_|AccountError::Storage)?;
                }
            } else {
                sqlx::query("INSERT OR IGNORE INTO catalog_conflicts(scope,device_id,desired_json) SELECT scope,device_id,item_json FROM account_catalog WHERE scope=? AND device_id=?").bind(scope).bind(mutation.device_id.to_string()).execute(&mut *tx).await.map_err(|_|AccountError::Storage)?;
                sqlx::query("UPDATE account_catalog SET dirty=0 WHERE scope=? AND device_id=?")
                    .bind(scope)
                    .bind(mutation.device_id.to_string())
                    .execute(&mut *tx)
                    .await
                    .map_err(|_| AccountError::Storage)?;
                sqlx::query("UPDATE catalog_sync SET cursor=0,error='device list changed on another client' WHERE scope=?").bind(scope).execute(&mut *tx).await.map_err(|_|AccountError::Storage)?;
            }
            sqlx::query("DELETE FROM catalog_outbox WHERE scope=? AND device_id=?")
                .bind(scope)
                .bind(mutation.device_id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(|_| AccountError::Storage)?;
            tx.commit().await.map_err(|_| AccountError::Storage)?;
        }
        // Give device-list work priority; a telemetry outage must not block it.
        if self.sync_usage(client, store).await.is_err() {
            tracing::debug!("usage upload pending");
        }
        Ok(())
    }
}
