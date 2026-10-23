use super::{PostgresStore, StoreError};
use crate::relay_cluster::RelayClusterConfig;
use pab_protocol::{RelayAdminState, RelayNodeControl, RelayNodeControlAck};
use sqlx::Row;

impl PostgresStore {
    pub async fn relay_node_control(&self, node: &str, boot: uuid::Uuid, instance: uuid::Uuid, config: &RelayClusterConfig, applied: Option<&RelayNodeControlAck>, connections: u64) -> Result<RelayNodeControl, StoreError> {
        let connections = i64::try_from(connections).map_err(|_| StoreError::InvalidInput("invalid connection count".into()))?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT n.admin_state,n.operation_id,COALESCE(n.state_applied_boot_id=n.boot_id AND n.state_applied_operation_id IS NOT DISTINCT FROM n.operation_id AND n.state_applied_admin_state=n.admin_state AND n.state_applied_at>clock_timestamp()-interval '15 seconds',false) was_applied,(extract(epoch from clock_timestamp())*1000)::bigint issued,(extract(epoch from COALESCE(o.deadline_at,n.maintenance_started_at+($4::bigint*interval '1 second')))*1000)::bigint drain_at,(SELECT revision FROM relay_cluster_state WHERE singleton) revision FROM relay_cluster_nodes n LEFT JOIN relay_maintenance_operations o ON o.id=n.operation_id WHERE n.node_id=$1 AND n.boot_id=$2 AND n.server_instance=$3 AND n.configured AND NOT n.retired FOR UPDATE OF n")
            .bind(node).bind(boot).bind(instance).bind(config.maintenance_seconds as i64).fetch_optional(&mut *tx).await?.ok_or(StoreError::PermissionDenied)?;
        let admin: &str = row.try_get("admin_state")?;
        let state = match admin { "active"=>RelayAdminState::Active,"maintenance"=>RelayAdminState::Maintenance,_=>RelayAdminState::Disabled };
        let operation_id: Option<uuid::Uuid> = row.try_get("operation_id")?;
        let issued: i64 = row.try_get("issued")?;
        let drain = if state==RelayAdminState::Maintenance { Some(row.try_get::<Option<i64>,_>("drain_at")?.unwrap_or(0)) } else {None};
        sqlx::query("UPDATE relay_cluster_nodes SET node_control_version=1,connections=$3 WHERE node_id=$1 AND boot_id=$2")
            .bind(node).bind(boot).bind(connections).execute(&mut *tx).await?;
        let mut activated=false;
        if let Some(ack) = applied {
            let matches = ack.operation_id==operation_id && ack.state==state && match state {
                RelayAdminState::Active => ack.accepting && ack.forwarding,
                RelayAdminState::Maintenance => !ack.accepting,
                RelayAdminState::Disabled => !ack.accepting && !ack.forwarding,
            };
            if matches {
                activated=state==RelayAdminState::Active && !row.try_get::<bool,_>("was_applied")?;
                let drained = !ack.accepting && !ack.forwarding && connections==0;
                sqlx::query("UPDATE relay_cluster_nodes SET state_applied_boot_id=$2,state_applied_operation_id=$3,state_applied_admin_state=$4,state_applied_at=clock_timestamp(),state_drained=$5 WHERE node_id=$1")
                    .bind(node).bind(boot).bind(operation_id).bind(admin).bind(drained).execute(&mut *tx).await?;
                // A deadline alone never proves that sockets were closed.
                let complete = state==RelayAdminState::Active || drained;
                if complete {
                    let phase = if state==RelayAdminState::Maintenance {"ready"} else {"completed"};
                    sqlx::query("UPDATE relay_maintenance_operations SET phase=$2,completed_at=CASE WHEN $2='completed' THEN clock_timestamp() END,error_code=NULL WHERE id=$1 AND node_id=$3 AND phase='waiting'")
                        .bind(operation_id).bind(phase).bind(node).execute(&mut *tx).await?;
                }
            }
        }
        let snapshot = RelayNodeControl {node_id:node.into(),boot_id:boot,revision:row.try_get::<i64,_>("revision")? as u64,operation_id,state,issued_at_unix_ms:issued,expires_at_unix_ms:issued+15_000,drain_at_unix_ms:drain};
        tx.commit().await?;
        if activated {
            // Do not acquire the catalog writer lock while holding a node lock.
            // The returned snapshot retains the revision of its own read.
            sqlx::query("UPDATE relay_cluster_state SET revision=revision+1 WHERE singleton").execute(self.pool()).await?;
        }
        Ok(snapshot)
    }
}
