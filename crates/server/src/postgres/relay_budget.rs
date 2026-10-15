use super::{PostgresStore, StoreError};
use crate::{relay_budget::packet_safe_shares, relay_cluster::RelayClusterConfig};
use pab_protocol::{
    MAX_RELAY_BUDGET_DEMANDS, RELAY_CLUSTER_SCHEMA_VERSION, RelayBudgetGrant, RelayBudgetSnapshot,
    UserId,
};
use sqlx::Row;
use time::OffsetDateTime;

impl PostgresStore {
    /// Each account is locked independently. Outstanding grants are immutable:
    /// a silent node keeps its reservation until expiry, even after restart.
    pub async fn relay_budgets(
        &self,
        node: &str,
        boot: uuid::Uuid,
        users: &[UserId],
        config: &RelayClusterConfig,
    ) -> Result<RelayBudgetSnapshot, StoreError> {
        let unique = users
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        if users.len() > MAX_RELAY_BUDGET_DEMANDS || unique.len() != users.len() {
            return Err(StoreError::InvalidInput(
                "invalid Relay budget batch".into(),
            ));
        }
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM relay_cluster_nodes WHERE node_id=$1 AND boot_id=$2 AND configured AND admin_state='active' AND health='healthy' AND last_seen_at>clock_timestamp()-($3::bigint*interval '1 second'))")
            .bind(node).bind(boot).bind(config.offline_seconds as i64).fetch_one(&self.pool).await?;
        if !active {
            return Err(StoreError::PermissionDenied);
        }
        // Most 1-second polls only confirm immutable 15-second leases. Batch
        // those reads/idle-demand updates rather than locking every account and
        // running an allocation transaction for each unchanged grant.
        let ids = users.iter().map(|user| user.as_uuid()).collect::<Vec<_>>();
        sqlx::query("UPDATE relay_budget_leases SET last_demand_at=clock_timestamp() WHERE node_id=$1 AND boot_id=$2 AND user_id=ANY($3::uuid[])")
            .bind(node).bind(boot).bind(&ids).execute(&self.pool).await?;
        let unchanged=sqlx::query("SELECT l.user_id,l.generation,l.bytes_per_second,l.burst_bytes,l.expires_at FROM relay_budget_leases l JOIN users u ON u.id=l.user_id WHERE l.node_id=$1 AND l.boot_id=$2 AND l.user_id=ANY($3::uuid[]) AND u.status='active' AND l.expires_at>clock_timestamp() AND l.bytes_per_second>0")
            .bind(node).bind(boot).bind(&ids).fetch_all(&self.pool).await?;
        let mut reusable = std::collections::HashMap::new();
        for row in unchanged {
            let user = UserId::from_uuid(row.try_get("user_id")?);
            let expires: OffsetDateTime = row.try_get("expires_at")?;
            reusable.insert(
                user,
                RelayBudgetGrant {
                    user_id: user,
                    generation: row.try_get::<i64, _>("generation")? as u64,
                    bytes_per_second: row.try_get::<i64, _>("bytes_per_second")? as u64,
                    burst_bytes: row.try_get::<i64, _>("burst_bytes")? as u64,
                    expires_at_unix_ms: (expires.unix_timestamp_nanos() / 1_000_000) as i64,
                },
            );
        }
        let mut grants = Vec::new();
        // Stable order prevents two batches locking accounts in opposite order.
        let mut ordered = users.to_vec();
        ordered.sort_by_key(|v| v.to_string());
        for user in ordered {
            if let Some(grant) = reusable.remove(&user) {
                grants.push(grant);
                continue;
            }
            let mut tx = self.pool.begin().await?;
            let mbps = sqlx::query_scalar::<_, i32>(
                "SELECT relay_limit_mbps FROM users WHERE id=$1 AND status='active' FOR UPDATE",
            )
            .bind(user.as_uuid())
            .fetch_optional(&mut *tx)
            .await?;
            let Some(mbps) = mbps else {
                continue;
            };
            let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
                .fetch_one(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO relay_budget_leases(user_id,node_id,boot_id,bytes_per_second,burst_bytes,expires_at) VALUES($1,$2,$3,0,0,$4) ON CONFLICT(user_id,node_id,boot_id) DO UPDATE SET last_demand_at=clock_timestamp()")
                .bind(user.as_uuid()).bind(node).bind(boot).bind(now).execute(&mut *tx).await?;
            let rows=sqlx::query("SELECT l.node_id,l.boot_id,l.generation,l.bytes_per_second,l.burst_bytes,l.expires_at,CASE WHEN n.configured AND n.admin_state='active' AND n.health='healthy' AND n.boot_id=l.boot_id AND n.last_seen_at>$2-($3::bigint*interval '1 second') AND l.last_demand_at>$2-interval '20 seconds' THEN n.weight ELSE 0 END AS demand_weight FROM relay_budget_leases l JOIN relay_cluster_nodes n USING(node_id) WHERE l.user_id=$1 ORDER BY l.node_id")
                .bind(user.as_uuid()).bind(now).bind(config.offline_seconds as i64).fetch_all(&mut *tx).await?;
            let total = (mbps as u64) * 1_000_000 / 8;
            let burst = total / 10;
            let mut reserved = 0u64;
            let mut reserved_burst = 0u64;
            let mut participants = Vec::new();
            let mut current = None;
            for row in &rows {
                let expires: OffsetDateTime = row.try_get("expires_at")?;
                let rate = row.try_get::<i64, _>("bytes_per_second")? as u64;
                let credit = row.try_get::<i64, _>("burst_bytes")? as u64;
                let id: String = row.try_get("node_id")?;
                let row_boot: uuid::Uuid = row.try_get("boot_id")?;
                let weight: i32 = row.try_get("demand_weight")?;
                if weight > 0 {
                    participants.push((
                        id.clone(),
                        weight as u32,
                        expires > now && rate > 0,
                        expires.unix_timestamp_nanos(),
                    ));
                }
                if expires > now {
                    reserved = reserved.saturating_add(rate);
                    reserved_burst = reserved_burst.saturating_add(credit);
                    if id == node && row_boot == boot && rate > 0 {
                        current = Some(RelayBudgetGrant {
                            user_id: user,
                            generation: row.try_get::<i64, _>("generation")? as u64,
                            bytes_per_second: rate,
                            burst_bytes: credit,
                            expires_at_unix_ms: (expires.unix_timestamp_nanos() / 1_000_000) as i64,
                        });
                    }
                }
            }
            if let Some(grant) = current {
                grants.push(grant);
                tx.commit().await?;
                continue;
            }
            let (shares, credits, lease_ms) = packet_safe_shares(total, &participants);
            let desired = shares.get(node).copied().unwrap_or(0);
            let desired_burst = credits.get(node).copied().unwrap_or(0);
            let rate = desired.min(total.saturating_sub(reserved));
            let credit = desired_burst.min(burst.saturating_sub(reserved_burst));
            if rate > 0 && credit > 0 {
                let expires = now + time::Duration::milliseconds(lease_ms);
                let generation:i64=sqlx::query_scalar("UPDATE relay_budget_leases SET bytes_per_second=$4,burst_bytes=$5,expires_at=$6,generation=nextval(pg_get_serial_sequence('relay_budget_leases','generation')) WHERE user_id=$1 AND node_id=$2 AND boot_id=$3 RETURNING generation")
                    .bind(user.as_uuid()).bind(node).bind(boot).bind(rate as i64).bind(credit as i64).bind(expires).fetch_one(&mut *tx).await?;
                grants.push(RelayBudgetGrant {
                    user_id: user,
                    generation: generation as u64,
                    bytes_per_second: rate,
                    burst_bytes: credit,
                    expires_at_unix_ms: (expires.unix_timestamp_nanos() / 1_000_000) as i64,
                });
            }
            tx.commit().await?;
        }
        let issued: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&self.pool)
            .await?;
        let revision: i64 = sqlx::query_scalar("SELECT nextval('relay_budget_snapshot_revision')")
            .fetch_one(&self.pool)
            .await?;
        Ok(RelayBudgetSnapshot {
            schema_version: RELAY_CLUSTER_SCHEMA_VERSION,
            node_id: node.into(),
            boot_id: boot,
            revision: revision as u64,
            issued_at_unix_ms: (issued.unix_timestamp_nanos() / 1_000_000) as i64,
            node_capacity_bytes_per_second: config
                .nodes
                .iter()
                .find(|n| n.node_id == node)
                .map(|n| u64::from(n.capacity_mbps) * 1_000_000 / 8)
                .unwrap_or(0),
            requested_users: users.to_vec(),
            grants,
        })
    }
}
