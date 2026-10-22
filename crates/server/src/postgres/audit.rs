use super::PostgresStore;
use crate::audit::{AuditRecord, AuditResource};

impl PostgresStore {
    pub async fn insert_audit(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        record: AuditRecord,
    ) -> Result<(), sqlx::Error> {
        let actor_name: Option<String> =
            sqlx::query_scalar("SELECT username FROM users WHERE id=$1")
                .bind(record.actor)
                .fetch_optional(&mut **tx)
                .await?;
        let (kind, key, id, name) = match record.resource {
            AuditResource::Account(id) => (
                "account",
                Some(id.to_string()),
                Some(id),
                sqlx::query_scalar::<_, String>("SELECT username FROM users WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut **tx)
                    .await?,
            ),
            AuditResource::Device(id) => (
                "device",
                Some(id.to_string()),
                Some(id),
                sqlx::query_scalar::<_, String>("SELECT name FROM devices WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut **tx)
                    .await?,
            ),
            AuditResource::Relay(node) => {
                ("relay", Some(node.clone()), record.operation_id, Some(node))
            }
            AuditResource::Server => ("server", Some("server".into()), None, Some("Server".into())),
            AuditResource::Audit => (
                "audit",
                Some("web-admin-events".into()),
                None,
                Some("Audit log".into()),
            ),
            AuditResource::Unknown => ("unknown", None, None, None),
        };
        let encode = |value| {
            serde_json::to_value(value)
                .map_err(|_| sqlx::Error::Protocol("invalid typed audit record".into()))
        };
        sqlx::query("INSERT INTO web_admin_events(actor_id,actor_ref,actor_name,action,resource_id,resource_kind,resource_key,resource_name,request_id,operation_id,result,before_values,after_values,details,format_version)
            VALUES($1,$1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,1)")
            .bind(record.actor).bind(actor_name).bind(record.action.as_str()).bind(id).bind(kind).bind(key).bind(name)
            .bind(record.request_id).bind(record.operation_id).bind(record.result)
            .bind(record.before.map(&encode).transpose()?).bind(record.after.map(&encode).transpose()?).bind(serde_json::to_value(record.details).map_err(|_|sqlx::Error::Protocol("invalid audit details".into()))?)
            .execute(&mut **tx).await?;
        Ok(())
    }

    pub async fn record_audit_failure(
        &self,
        mut record: AuditRecord,
        class: &'static str,
    ) -> Result<bool, sqlx::Error> {
        record.result = if matches!(class, "unauthorized" | "forbidden") {
            "denied"
        } else {
            "failed"
        };
        let mut tx = self.begin_snapshot(false).await?;
        let permitted = sqlx::query("INSERT INTO audit_failure_limits(actor_key,action,error_class,recorded_at) VALUES($1,$2,$3,clock_timestamp())
            ON CONFLICT(actor_key,action,error_class) DO UPDATE SET recorded_at=excluded.recorded_at
            WHERE audit_failure_limits.recorded_at<=clock_timestamp()-interval '1 minute' RETURNING actor_key")
            .bind(record.actor.unwrap_or(uuid::Uuid::nil())).bind(record.action.as_str()).bind(class).fetch_optional(&mut *tx).await?.is_some();
        if permitted {
            self.insert_audit(&mut tx, record).await?;
        }
        tx.commit().await?;
        Ok(permitted)
    }
}
