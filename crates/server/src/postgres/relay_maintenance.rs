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
    pub remaining_capacity_mbps: Option<i64>,
    pub remaining_host_bandwidth: bool,
    pub capacity_sampled_at_unix_ms: i64,
    pub capacity_source: &'static str,
    pub server_now_unix_ms: i64,
    pub started_at_unix_ms: i64,
    pub completed_at_unix_ms: Option<i64>,
    pub actor_id: uuid::Uuid,
    pub actor: String,
    pub error_code: Option<String>,
    pub allow_outage: bool,
}
impl PostgresStore {
    pub async fn relay_maintenance_operation(
        &self,
        id: uuid::Uuid,
    ) -> Result<RelayMaintenanceOperation, StoreError> {
        self.relay_maintenance_operation_in_instance(id, None).await
    }

    pub async fn relay_maintenance_operation_in_instance(
        &self,
        id: uuid::Uuid,
        server_instance: Option<uuid::Uuid>,
    ) -> Result<RelayMaintenanceOperation, StoreError> {
        // Polling is sufficient to expire an abandoned operation. It never changes
        // the node's administrative state or claims that an upgrade has run.
        sqlx::query("UPDATE relay_maintenance_operations SET phase=CASE WHEN deadline_at<clock_timestamp()-interval '5 minutes' THEN 'failed' ELSE 'ready' END,completed_at=CASE WHEN deadline_at<clock_timestamp()-interval '5 minutes' THEN clock_timestamp() END,error_code=CASE WHEN deadline_at<clock_timestamp()-interval '5 minutes' THEN 'operator_lease_expired' END WHERE id=$1 AND phase IN ('waiting','ready') AND ((phase='waiting' AND deadline_at<=clock_timestamp()) OR deadline_at<clock_timestamp()-interval '5 minutes')")
            .bind(id).execute(&self.pool).await?;
        let row=sqlx::query("SELECT o.*,u.username actor,clock_timestamp() sampled_at,CASE WHEN o.offline_seconds IS NOT NULL AND ($2::uuid IS NULL OR o.server_instance=$2) THEN (SELECT CASE WHEN bool_or(n.capacity_mbps=0) THEN NULL ELSE COALESCE(sum(n.capacity_mbps),0)::bigint END FROM relay_cluster_nodes n WHERE n.node_id<>o.node_id AND n.configured AND n.admin_state='active' AND n.health='healthy' AND ($2::uuid IS NULL OR n.server_instance=$2) AND n.last_seen_at>clock_timestamp()-(o.offline_seconds*interval '1 second')) END remaining_capacity_mbps, COALESCE(o.offline_seconds IS NOT NULL AND ($2::uuid IS NULL OR o.server_instance=$2) AND EXISTS(SELECT 1 FROM relay_cluster_nodes n WHERE n.node_id<>o.node_id AND n.configured AND n.admin_state='active' AND n.health='healthy' AND n.capacity_mbps=0 AND ($2::uuid IS NULL OR n.server_instance=$2) AND n.last_seen_at>clock_timestamp()-(o.offline_seconds*interval '1 second')),false) remaining_host_bandwidth FROM relay_maintenance_operations o JOIN users u ON u.id=o.actor_id WHERE o.id=$1")
            .bind(id).bind(server_instance).fetch_optional(&self.pool).await?.ok_or(StoreError::NotFound)?;
        let deadline: OffsetDateTime = row.try_get("deadline_at")?;
        let sampled_at: OffsetDateTime = row.try_get("sampled_at")?;
        let started_at: OffsetDateTime = row.try_get("started_at")?;
        let completed_at: Option<OffsetDateTime> = row.try_get("completed_at")?;
        let sampled_at_ms = (sampled_at.unix_timestamp_nanos() / 1_000_000) as i64;
        Ok(RelayMaintenanceOperation {
            id,
            node_id: row.try_get("node_id")?,
            phase: row.try_get("phase")?,
            action: row.try_get("action")?,
            deadline_unix_ms: (deadline.unix_timestamp_nanos() / 1_000_000) as i64,
            remaining_capacity_mbps: row.try_get("remaining_capacity_mbps")?,
            remaining_host_bandwidth: row.try_get("remaining_host_bandwidth")?,
            capacity_sampled_at_unix_ms: sampled_at_ms,
            capacity_source: "healthy_active_peers",
            server_now_unix_ms: sampled_at_ms,
            started_at_unix_ms: (started_at.unix_timestamp_nanos() / 1_000_000) as i64,
            completed_at_unix_ms: completed_at
                .map(|value| (value.unix_timestamp_nanos() / 1_000_000) as i64),
            actor_id: row.try_get("actor_id")?,
            actor: row.try_get("actor")?,
            error_code: row.try_get("error_code")?,
            allow_outage: row.try_get("allow_outage")?,
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
        self.set_relay_state_in_instance(
            node,
            desired,
            operation,
            actor,
            allow_outage,
            wait_seconds,
            offline_seconds,
            None,
        )
        .await
    }

    pub async fn set_relay_state_in_instance(
        &self,
        node: &str,
        desired: RelayAdminState,
        operation: uuid::Uuid,
        actor: uuid::Uuid,
        allow_outage: bool,
        wait_seconds: u64,
        offline_seconds: u64,
        server_instance: Option<uuid::Uuid>,
    ) -> Result<RelayMaintenanceOperation, StoreError> {
        self.set_relay_state_audited(
            node,
            desired,
            operation,
            actor,
            allow_outage,
            wait_seconds,
            offline_seconds,
            server_instance,
            operation,
        )
        .await
    }

    pub(crate) async fn set_relay_state_audited(
        &self,
        node: &str,
        desired: RelayAdminState,
        operation: uuid::Uuid,
        actor: uuid::Uuid,
        allow_outage: bool,
        wait_seconds: u64,
        offline_seconds: u64,
        server_instance: Option<uuid::Uuid>,
        request_id: uuid::Uuid,
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
            return self
                .relay_maintenance_operation_in_instance(operation, server_instance)
                .await;
        }
        // An abandoned upgrade must not permanently lock unrelated nodes.
        // Its node remains in maintenance; it is never silently reactivated.
        sqlx::query("UPDATE relay_maintenance_operations SET phase='failed',completed_at=clock_timestamp(),error_code='operator_lease_expired' WHERE phase IN ('waiting','ready') AND deadline_at < clock_timestamp()-interval '5 minutes'")
            .execute(&mut *tx).await?;
        let row=sqlx::query("SELECT configured,health,admin_state,last_seen_at>clock_timestamp()-($2::bigint*interval '1 second') AND ($3::uuid IS NULL OR server_instance=$3) AS fresh FROM relay_cluster_nodes WHERE node_id=$1 FOR UPDATE")
            .bind(node).bind(offline_seconds as i64).bind(server_instance).fetch_optional(&mut *tx).await?.ok_or(StoreError::NotFound)?;
        if desired == RelayAdminState::Active
            && (!row.try_get::<bool, _>("configured")?
                || row.try_get::<&str, _>("health")? != "healthy"
                || !row.try_get::<Option<bool>, _>("fresh")?.unwrap_or(false))
        {
            return Err(StoreError::InvalidState(
                "Relay is not healthy and cannot be activated".into(),
            ));
        }
        let remaining:i64=sqlx::query_scalar("SELECT count(*)::bigint FROM relay_cluster_nodes WHERE node_id<>$1 AND configured AND admin_state='active' AND health='healthy' AND ($3::uuid IS NULL OR server_instance=$3) AND last_seen_at>clock_timestamp()-($2::bigint*interval '1 second')")
            .bind(node).bind(offline_seconds as i64).bind(server_instance).fetch_one(&mut *tx).await?;
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
        sqlx::query("INSERT INTO relay_maintenance_operations(id,node_id,action,phase,allow_outage,deadline_at,actor_id,completed_at,offline_seconds,server_instance) VALUES($1,$2,$3,$4,$5,clock_timestamp()+($6::bigint*interval '1 second'),$7,CASE WHEN $4='completed' THEN clock_timestamp() ELSE NULL END,$8,$9)")
            .bind(operation).bind(node).bind(action).bind(phase).bind(allow_outage).bind(wait_seconds as i64).bind(actor).bind(offline_seconds as i64).bind(server_instance).execute(&mut *tx).await?;
        sqlx::query("UPDATE relay_cluster_state SET revision=revision+1 WHERE singleton")
            .execute(&mut *tx)
            .await?;
        let mut record = crate::audit::AuditRecord::new(
            Some(actor),
            match desired {
                RelayAdminState::Active => crate::audit::AuditAction::RelayActivate,
                RelayAdminState::Maintenance => crate::audit::AuditAction::RelayMaintenance,
                RelayAdminState::Disabled => crate::audit::AuditAction::RelayDisable,
            },
            crate::audit::AuditResource::Relay(node.into()),
        );
        record.request_id = request_id;
        record.operation_id = Some(operation);
        record.before = Some(crate::audit::AuditValues {
            status: Some(row.try_get("admin_state")?),
            ..Default::default()
        });
        record.after = Some(crate::audit::AuditValues {
            status: Some(admin.into()),
            ..Default::default()
        });
        record.details.allow_outage = Some(allow_outage);
        record.details.phase = Some(phase.into());
        self.insert_audit(&mut tx, record).await?;
        tx.commit().await?;
        self.relay_maintenance_operation_in_instance(operation, server_instance)
            .await
    }
}
