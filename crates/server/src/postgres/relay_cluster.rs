use super::{PostgresStore, StoreError};
use crate::relay_cluster::RelayClusterConfig;
use pab_protocol::{
    RELAY_CLUSTER_SCHEMA_VERSION, RelayAdminState, RelayDirectory, RelayDirectoryNode, RelayHealth,
};
use sqlx::Row;

impl PostgresStore {
    pub async fn retire_relay(&self, node: &str, boot: uuid::Uuid) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT revision FROM relay_cluster_state WHERE singleton FOR UPDATE")
            .fetch_one(&mut *tx)
            .await?;
        let retired = sqlx::query_scalar::<_, bool>(
            "SELECT retired FROM relay_cluster_nodes WHERE node_id=$1 AND boot_id=$2 FOR UPDATE",
        )
        .bind(node)
        .bind(boot)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::PermissionDenied)?;
        if !retired {
            sqlx::query("UPDATE relay_cluster_nodes SET retired=true,health='offline',healthy_since=NULL WHERE node_id=$1 AND boot_id=$2")
                .bind(node).bind(boot).execute(&mut *tx).await?;
            // Forwarding has already stopped. Keep a burst-refill interval
            // reserved before making that budget available to another node.
            sqlx::query("UPDATE relay_budget_leases SET expires_at=LEAST(expires_at,clock_timestamp()+interval '100 milliseconds') WHERE node_id=$1 AND boot_id=$2")
                .bind(node).bind(boot).execute(&mut *tx).await?;
            sqlx::query("UPDATE relay_cluster_state SET revision=revision+1 WHERE singleton")
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn register_relay_boot(
        &self,
        node_id: &str,
        boot_id: uuid::Uuid,
        version: &str,
        compatible: bool,
        config: &RelayClusterConfig,
        instance: uuid::Uuid,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        let enabled: bool = sqlx::query_scalar(
            "SELECT enabled FROM relay_cluster_state WHERE singleton FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        if !enabled {
            return Err(StoreError::PermissionDenied);
        }
        let row = sqlx::query("SELECT boot_id,retired,configured,admin_state,last_seen_at > clock_timestamp()-($2::bigint*interval '1 second') AS alive FROM relay_cluster_nodes WHERE node_id=$1 FOR UPDATE")
            .bind(node_id).bind(config.offline_seconds as i64).fetch_optional(&mut *tx).await?.ok_or(StoreError::PermissionDenied)?;
        if !row.try_get::<bool, _>("configured")?
            || row.try_get::<&str, _>("admin_state")? == "disabled"
        {
            return Err(StoreError::PermissionDenied);
        }
        let old: Option<uuid::Uuid> = row.try_get("boot_id")?;
        let retired: bool = row.try_get("retired")?;
        if old == Some(boot_id) && retired {
            return Err(StoreError::PermissionDenied);
        }
        if !retired
            && old.is_some_and(|old| old != boot_id)
            && row.try_get::<Option<bool>, _>("alive")?.unwrap_or(false)
        {
            return Err(StoreError::Conflict(
                "Relay node already has a live process",
            ));
        }
        let health = if compatible {
            "starting"
        } else {
            "incompatible"
        };
        let changed = sqlx::query("UPDATE relay_cluster_nodes SET boot_id=$2,retired=false,agent_version=$3,health=$4,healthy_since=NULL,last_seen_at=clock_timestamp(),server_instance=$5 WHERE node_id=$1 AND (boot_id IS DISTINCT FROM $2 OR health='offline' OR health='incompatible' OR server_instance IS DISTINCT FROM $5 OR NOT $6)")
            .bind(node_id).bind(boot_id).bind(version).bind(health).bind(instance).bind(compatible).execute(&mut *tx).await?.rows_affected();
        if changed > 0 {
            sqlx::query("UPDATE relay_cluster_state SET revision=revision+1 WHERE singleton")
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn heartbeat_relay(
        &self,
        node_id: &str,
        boot_id: uuid::Uuid,
        policy_applied: bool,
        config: &RelayClusterConfig,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT revision FROM relay_cluster_state WHERE singleton FOR UPDATE")
            .fetch_one(&mut *tx)
            .await?;
        let row = sqlx::query("SELECT health,healthy_since IS NOT NULL AND healthy_since <= clock_timestamp()-($3::bigint*interval '1 second') AS recovered FROM relay_cluster_nodes WHERE node_id=$1 AND boot_id=$2 AND NOT retired AND configured AND admin_state<>'disabled' FOR UPDATE")
            .bind(node_id).bind(boot_id).bind(config.recovery_seconds as i64).fetch_optional(&mut *tx).await?.ok_or(StoreError::PermissionDenied)?;
        let old: &str = row.try_get("health")?;
        let health = if policy_applied && row.try_get::<bool, _>("recovered")? {
            "healthy"
        } else if old == "healthy" && policy_applied {
            "healthy"
        } else {
            "starting"
        };
        sqlx::query("UPDATE relay_cluster_nodes SET health=$3,last_seen_at=clock_timestamp(),healthy_since=CASE WHEN $4 THEN COALESCE(healthy_since,clock_timestamp()) ELSE NULL END WHERE node_id=$1 AND boot_id=$2")
            .bind(node_id).bind(boot_id).bind(health).bind(policy_applied).execute(&mut *tx).await?;
        if health != old {
            sqlx::query("UPDATE relay_cluster_state SET revision=revision+1 WHERE singleton")
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn configure_relay_cluster(
        &self,
        config: &RelayClusterConfig,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        let enabled: bool = sqlx::query_scalar(
            "SELECT enabled FROM relay_cluster_state WHERE singleton FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        if !enabled {
            let legacy_alive:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM relay_nodes WHERE last_seen_at>clock_timestamp()-interval '60 seconds')").fetch_one(&mut *tx).await?;
            if legacy_alive {
                return Err(StoreError::InvalidState("stop legacy Relays and wait for their policy lifetime before enabling cluster mode".into()));
            }
        }
        sqlx::query("UPDATE relay_cluster_nodes SET configured=false,admin_state=CASE WHEN admin_state='active' THEN 'maintenance' ELSE admin_state END,maintenance_started_at=COALESCE(maintenance_started_at,clock_timestamp()) WHERE NOT(node_id=ANY($1))")
            .bind(config.nodes.iter().map(|n| n.node_id.clone()).collect::<Vec<_>>()).execute(&mut *tx).await?;
        for node in &config.nodes {
            sqlx::query("INSERT INTO relay_cluster_nodes(node_id,public_url,region,weight,capacity_mbps) VALUES($1,$2,$3,$4,$5) ON CONFLICT(node_id) DO UPDATE SET public_url=excluded.public_url,region=excluded.region,weight=excluded.weight,capacity_mbps=excluded.capacity_mbps,configured=true")
                .bind(&node.node_id).bind(&node.url).bind(&node.region).bind(node.weight as i32)
                .bind(i32::try_from(node.capacity_mbps).map_err(|_| StoreError::InvalidInput("node capacity exceeds storage range".into()))?)
                .execute(&mut *tx).await?;
        }
        sqlx::query(
            "UPDATE relay_cluster_state SET revision=revision+1,enabled=true WHERE singleton",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn disable_relay_cluster(&self) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        let enabled: bool = sqlx::query_scalar(
            "SELECT enabled FROM relay_cluster_state WHERE singleton FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        if enabled {
            let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM relay_budget_leases WHERE expires_at>clock_timestamp() AND bytes_per_second>0)").fetch_one(&mut *tx).await?;
            if live {
                return Err(StoreError::InvalidState("cluster budgets are still active; stop cluster Relays and wait for lease expiry before rollback".into()));
            }
            sqlx::query(
                "UPDATE relay_cluster_state SET revision=revision+1,enabled=false WHERE singleton",
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn relay_directory(
        &self,
        offline_seconds: u64,
    ) -> Result<RelayDirectory, StoreError> {
        // Healthy directory polls must not queue behind the global writer lock:
        // many endpoints can otherwise consume the whole pool while heartbeat
        // and registration wait for that same row. Only expiry changes write.
        let stale: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM relay_cluster_nodes WHERE health IN ('healthy','starting','suspect') AND last_seen_at < clock_timestamp()-($1::bigint*interval '1 second'))")
            .bind(offline_seconds as i64).fetch_one(&self.pool).await?;
        if stale {
            let mut tx = self.pool.begin().await?;
            sqlx::query("SELECT revision FROM relay_cluster_state WHERE singleton FOR UPDATE")
                .fetch_one(&mut *tx)
                .await?;
            let expired = sqlx::query("UPDATE relay_cluster_nodes SET health='offline',healthy_since=NULL WHERE health IN ('healthy','starting','suspect') AND last_seen_at < clock_timestamp()-($1::bigint*interval '1 second')")
                .bind(offline_seconds as i64).execute(&mut *tx).await?.rows_affected();
            if expired > 0 {
                sqlx::query("UPDATE relay_cluster_state SET revision=revision+1 WHERE singleton")
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
        }
        // One SQL statement gives revision AND rows the same MVCC snapshot,
        // without a read transaction or row lock. LEFT JOIN preserves the
        // revision when the configured directory is empty.
        let rows = sqlx::query("SELECT s.revision,n.node_id,n.public_url,n.region,n.weight,n.capacity_mbps,n.admin_state,n.health FROM relay_cluster_state s LEFT JOIN relay_cluster_nodes n ON n.configured WHERE s.singleton ORDER BY n.node_id")
            .fetch_all(&self.pool).await?;
        let revision: i64 = rows
            .first()
            .ok_or_else(|| StoreError::InvalidState("Relay cluster state is missing".into()))?
            .try_get("revision")?;
        let mut nodes = Vec::with_capacity(rows.len());
        for row in rows {
            if row.try_get::<Option<String>, _>("node_id")?.is_none() {
                continue;
            }
            let admin_state = match row.try_get::<&str, _>("admin_state")? {
                "active" => RelayAdminState::Active,
                "maintenance" => RelayAdminState::Maintenance,
                _ => RelayAdminState::Disabled,
            };
            let health = match row.try_get::<&str, _>("health")? {
                "healthy" => RelayHealth::Healthy,
                "suspect" => RelayHealth::Suspect,
                "offline" => RelayHealth::Offline,
                "incompatible" => RelayHealth::Incompatible,
                _ => RelayHealth::Starting,
            };
            nodes.push(RelayDirectoryNode {
                node_id: row.try_get("node_id")?,
                url: row.try_get("public_url")?,
                region: row.try_get("region")?,
                weight: row.try_get::<i32, _>("weight")? as u32,
                capacity_mbps: row.try_get::<i32, _>("capacity_mbps")? as u32,
                admin_state,
                health,
            });
        }
        Ok(RelayDirectory {
            schema_version: RELAY_CLUSTER_SCHEMA_VERSION,
            revision: revision as u64,
            nodes,
        })
    }
}
