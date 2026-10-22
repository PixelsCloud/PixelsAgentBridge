use super::{PostgresStore, StoreError};
use crate::{audit::{AuditAction, AuditRecord, AuditResource, AuditValues}, relay_cluster::RelayClusterConfig};
use sqlx::Row;

impl PostgresStore {
    pub(crate) async fn apply_relay_capacity_overrides(&self, config: &mut RelayClusterConfig) -> Result<(), StoreError> {
        let overrides: Vec<(String, i32)> = sqlx::query_as("SELECT node_id,capacity_override_mbps FROM relay_cluster_nodes WHERE capacity_override_mbps IS NOT NULL")
            .fetch_all(self.pool()).await?;
        for node in &mut config.nodes {
            if let Some((_, capacity)) = overrides.iter().find(|(id, _)| id == &node.node_id) {
                node.capacity_mbps = *capacity as u32;
            }
        }
        Ok(())
    }

    pub(crate) async fn set_relay_capacity_audited(&self, node: &str, capacity: u32, machine_ip: Option<String>, actor: uuid::Uuid, request_id: uuid::Uuid) -> Result<(), StoreError> {
        let capacity = i32::try_from(capacity).ok().filter(|v| *v > 0)
            .ok_or_else(|| StoreError::InvalidInput("node capacity must be positive Mbps".into()))?;
        let mut tx = self.pool().begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(26035001)").execute(&mut *tx).await?;
        let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND status='active' AND server_admin)")
            .bind(actor).fetch_one(&mut *tx).await?;
        if !allowed { return Err(StoreError::PermissionDenied); }
        sqlx::query("SELECT revision FROM relay_cluster_state WHERE singleton FOR UPDATE").execute(&mut *tx).await?;
        let old = sqlx::query("SELECT capacity_mbps,host(machine_ip) machine_ip FROM relay_cluster_nodes WHERE node_id=$1 AND configured FOR UPDATE")
            .bind(node).fetch_optional(&mut *tx).await?.ok_or(StoreError::NotFound)?;
        sqlx::query("UPDATE relay_cluster_nodes SET capacity_mbps=$2,capacity_override_mbps=$2,machine_ip=CASE WHEN $3::text IS NULL THEN machine_ip ELSE NULLIF($3,'')::inet END WHERE node_id=$1")
            .bind(node).bind(capacity).bind(&machine_ip).execute(&mut *tx).await?;
        sqlx::query("UPDATE relay_cluster_state SET revision=revision+1 WHERE singleton").execute(&mut *tx).await?;
        let mut audit = AuditRecord::new(Some(actor), AuditAction::RelayCapacityUpdated, AuditResource::Relay(node.into()));
        audit.request_id = request_id;
        let old_ip: Option<String> = old.try_get("machine_ip")?;
        let new_ip = machine_ip.map(|ip| if ip.is_empty() { None } else { Some(ip) }).unwrap_or_else(|| old_ip.clone());
        audit.before = Some(AuditValues { mbps: Some(old.try_get("capacity_mbps")?), machine_ip: old_ip, ..Default::default() });
        audit.after = Some(AuditValues { mbps: Some(capacity), machine_ip: new_ip, ..Default::default() });
        self.insert_audit(&mut tx, audit).await?;
        tx.commit().await?;
        Ok(())
    }
}
