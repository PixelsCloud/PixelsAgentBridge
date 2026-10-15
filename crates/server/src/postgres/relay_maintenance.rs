use super::{PostgresStore, StoreError};
use pab_protocol::RelayAdminState;
use sqlx::Row;
use time::OffsetDateTime;

#[derive(Debug, Clone, serde::Serialize)]
pub struct RelayMaintenanceOperation {
    pub id: uuid::Uuid,
    pub node_id: String,
    pub phase: String,
    pub action: String,
    pub deadline_unix_ms: i64,
    pub remaining_capacity_mbps: i64,
}
impl PostgresStore {
    pub async fn relay_maintenance_operation(
        &self,
        id: uuid::Uuid,
    ) -> Result<RelayMaintenanceOperation, StoreError> {
        sqlx::query("UPDATE relay_maintenance_operations SET phase='ready' WHERE id=$1 AND phase='waiting' AND deadline_at<=clock_timestamp()")
            .bind(id).execute(&self.pool).await?;
        let row=sqlx::query("SELECT id,node_id,action,phase,deadline_at FROM relay_maintenance_operations WHERE id=$1")
            .bind(id).fetch_optional(&self.pool).await?.ok_or(StoreError::NotFound)?;
        let deadline: OffsetDateTime = row.try_get("deadline_at")?;
        Ok(RelayMaintenanceOperation {
            id,
            node_id: row.try_get("node_id")?,
            phase: row.try_get("phase")?,
            action: row.try_get("action")?,
            deadline_unix_ms: (deadline.unix_timestamp_nanos() / 1_000_000) as i64,
            remaining_capacity_mbps: 0,
        })
    }

    pub async fn set_relay_state(
        &self,
        node: &str,
        desired: RelayAdminState,
        operation: uuid::Uuid,
        actor: uuid::Uuid,
        allow_outage: bool,
        wait_seconds: u64,
        offline_seconds: u64,
    ) -> Result<RelayMaintenanceOperation, StoreError> {
        let action = match desired {
            RelayAdminState::Active => "activate",
            RelayAdminState::Maintenance => "maintenance",
            RelayAdminState::Disabled => "disable",
        };
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT revision FROM relay_cluster_state WHERE singleton FOR UPDATE")
            .fetch_one(&mut *tx)
            .await?;
        if let Some(row) = sqlx::query(
            "SELECT node_id,action,allow_outage FROM relay_maintenance_operations WHERE id=$1",
        )
        .bind(operation)
        .fetch_optional(&mut *tx)
        .await?
        {
            if row.try_get::<&str, _>("node_id")? != node
                || row.try_get::<&str, _>("action")? != action
                || row.try_get::<bool, _>("allow_outage")? != allow_outage
            {
                return Err(StoreError::Conflict(
                    "operation ID already belongs to another action",
                ));
            }
            tx.commit().await?;
            return self.relay_maintenance_operation(operation).await;
        }
        // An abandoned upgrade must not permanently lock unrelated nodes.
        // Its node remains in maintenance; it is never silently reactivated.
        sqlx::query("UPDATE relay_maintenance_operations SET phase='failed',completed_at=clock_timestamp(),error_code='operator_lease_expired' WHERE phase IN ('waiting','ready') AND deadline_at < clock_timestamp()-interval '5 minutes'")
            .execute(&mut *tx).await?;
        let row=sqlx::query("SELECT configured,health,last_seen_at>clock_timestamp()-($2::bigint*interval '1 second') AS fresh FROM relay_cluster_nodes WHERE node_id=$1 FOR UPDATE")
            .bind(node).bind(offline_seconds as i64).fetch_optional(&mut *tx).await?.ok_or(StoreError::NotFound)?;
        if desired == RelayAdminState::Active
            && (!row.try_get::<bool, _>("configured")?
                || row.try_get::<&str, _>("health")? != "healthy"
                || !row.try_get::<Option<bool>, _>("fresh")?.unwrap_or(false))
        {
            return Err(StoreError::InvalidState(
                "Relay is not healthy and cannot be activated".into(),
            ));
        }
        let remaining:i64=sqlx::query_scalar("SELECT COALESCE(sum(capacity_mbps),0)::bigint FROM relay_cluster_nodes WHERE node_id<>$1 AND configured AND admin_state='active' AND health='healthy' AND last_seen_at>clock_timestamp()-($2::bigint*interval '1 second')")
            .bind(node).bind(offline_seconds as i64).fetch_one(&mut *tx).await?;
        if desired != RelayAdminState::Active && remaining == 0 && !allow_outage {
            return Err(StoreError::InvalidState(
                "no healthy Relay backup; explicit allow_outage is required".into(),
            ));
        }
        if desired == RelayAdminState::Maintenance {
            let other:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM relay_maintenance_operations WHERE phase IN ('waiting','ready'))").fetch_one(&mut *tx).await?;
            if other {
                return Err(StoreError::Conflict("another Relay is being maintained"));
            }
        } else {
            sqlx::query("UPDATE relay_maintenance_operations SET phase='completed',completed_at=clock_timestamp() WHERE node_id=$1 AND phase IN ('waiting','ready')")
                .bind(node).execute(&mut *tx).await?;
        }
        let admin = match desired {
            RelayAdminState::Active => "active",
            RelayAdminState::Maintenance => "maintenance",
            RelayAdminState::Disabled => "disabled",
        };
        sqlx::query("UPDATE relay_cluster_nodes SET admin_state=$2,maintenance_started_at=CASE WHEN $2='maintenance' THEN clock_timestamp() ELSE NULL END,operation_id=$3 WHERE node_id=$1")
            .bind(node).bind(admin).bind(operation).execute(&mut *tx).await?;
        let phase = if desired == RelayAdminState::Maintenance {
            "waiting"
        } else {
            "completed"
        };
        let deadline:OffsetDateTime=sqlx::query_scalar("INSERT INTO relay_maintenance_operations(id,node_id,action,phase,allow_outage,deadline_at,actor_id,completed_at) VALUES($1,$2,$3,$4,$5,clock_timestamp()+($6::bigint*interval '1 second'),$7,CASE WHEN $4='completed' THEN clock_timestamp() ELSE NULL END) RETURNING deadline_at")
            .bind(operation).bind(node).bind(action).bind(phase).bind(allow_outage).bind(wait_seconds as i64).bind(actor).fetch_one(&mut *tx).await?;
        sqlx::query("UPDATE relay_cluster_state SET revision=revision+1 WHERE singleton")
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO web_admin_events(actor_id,action,resource_id) VALUES($1,$2,$3)")
            .bind(actor)
            .bind(format!("relay_{action}"))
            .bind(operation)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(RelayMaintenanceOperation {
            id: operation,
            node_id: node.into(),
            phase: phase.into(),
            action: action.into(),
            deadline_unix_ms: (deadline.unix_timestamp_nanos() / 1_000_000) as i64,
            remaining_capacity_mbps: remaining,
        })
    }
}
