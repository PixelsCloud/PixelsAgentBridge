use super::{TaskStore, TaskStoreError};
use pab_protocol::{OperatorRef, RequestId, SystemQuery, SystemQueryReply};
use sqlx::Row;

impl TaskStore {
    pub async fn accept_system_query(
        &self,
        actor: OperatorRef,
        id: RequestId,
        query: &SystemQuery,
    ) -> Result<bool, TaskStoreError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let prior=sqlx::query("SELECT o.initiated_by_json, q.query_json FROM read_operations o LEFT JOIN system_query_results q ON q.request_id=o.request_id WHERE o.request_id=?").bind(id.to_string()).fetch_optional(&mut *tx).await?;
        if let Some(row) = prior {
            if serde_json::from_str::<OperatorRef>(row.try_get("initiated_by_json")?)? != actor {
                return Err(TaskStoreError::NotFound);
            }
            if row.try_get::<Option<String>, _>("query_json")?.is_none() {
                return Err(TaskStoreError::RequestConflict);
            }
            if serde_json::from_str::<SystemQuery>(row.try_get("query_json")?)?
                != query.persistence_form()
            {
                return Err(TaskStoreError::RequestConflict);
            }
            return Ok(false);
        }
        sqlx::query("INSERT INTO read_operations (request_id,initiated_by_json,kind,path,state,started_at_unix_ms) VALUES (?,?,?,'','running',?)").bind(id.to_string()).bind(serde_json::to_string(&actor)?).bind(query.kind()).bind(super::operation::now_unix_ms()).execute(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO system_query_results (request_id,query_json,reply_json) VALUES (?,?,?)",
        )
        .bind(id.to_string())
        .bind(serde_json::to_string(&query.persistence_form())?)
        .bind(serde_json::to_string(&SystemQueryReply::pending(
            id, query,
        ))?)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }
    pub async fn get_system_query(
        &self,
        actor: OperatorRef,
        id: RequestId,
    ) -> Result<SystemQueryReply, TaskStoreError> {
        let row=sqlx::query("SELECT o.initiated_by_json,o.state,o.message,q.reply_json FROM read_operations o JOIN system_query_results q ON q.request_id=o.request_id WHERE o.request_id=?").bind(id.to_string()).fetch_optional(&self.pool).await?.ok_or(TaskStoreError::NotFound)?;
        if serde_json::from_str::<OperatorRef>(row.try_get("initiated_by_json")?)? != actor {
            return Err(TaskStoreError::NotFound);
        }
        let mut r: SystemQueryReply = serde_json::from_str(row.try_get("reply_json")?)?;
        r.state = row.try_get("state")?;
        if r.error.is_none() {
            r.error = row.try_get("message")?;
        }
        Ok(r)
    }
    pub async fn finish_system_query(&self, r: &SystemQueryReply) -> Result<(), TaskStoreError> {
        let mut tx = self.pool.begin().await?;
        let result=sqlx::query("UPDATE read_operations SET state=?, result_count=?, finished_at_unix_ms=?, message=? WHERE request_id=? AND state IN ('running','unconfirmed')").bind(&r.state).bind(r.returned_count as i64).bind(super::operation::now_unix_ms()).bind(&r.error).bind(r.request_id.to_string()).execute(&mut *tx).await?;
        if result.rows_affected() == 1 {
            sqlx::query("UPDATE system_query_results SET reply_json=? WHERE request_id=?")
                .bind(serde_json::to_string(r)?)
                .bind(r.request_id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_protocol::*;
    #[tokio::test]
    async fn text_identity_is_private_deduplicated_owned_and_unconfirmed_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.sqlite3");
        let store = TaskStore::open(&path).await.unwrap();
        let actor = crate::task_service::transfer_tests::actor();
        let id = RequestId::new();
        let q = SystemQuery::Desktop {
            query: DesktopQuery::TypeText {
                window_ref: RequestId::new().to_string(),
                text: "secret中文🙂".into(),
            },
        };
        assert!(store.accept_system_query(actor, id, &q).await.unwrap());
        assert!(!store.accept_system_query(actor, id, &q).await.unwrap());
        let saved: String =
            sqlx::query_scalar("SELECT query_json FROM system_query_results WHERE request_id=?")
                .bind(id.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(!saved.contains("secret"));
        assert!(saved.contains("blake3:"));
        let mut changed = q.clone();
        if let SystemQuery::Desktop {
            query: DesktopQuery::TypeText { text, .. },
        } = &mut changed
        {
            *text = "different".into();
        }
        assert!(matches!(
            store.accept_system_query(actor, id, &changed).await,
            Err(TaskStoreError::RequestConflict)
        ));
        let other = OperatorRef::account(UserId::from_u128(900), EndpointKey::new([90; 32]));
        assert!(matches!(
            store.get_system_query(other, id).await,
            Err(TaskStoreError::NotFound)
        ));
        assert!(matches!(
            store.accept_system_query(other, id, &q).await,
            Err(TaskStoreError::NotFound)
        ));
        store.interrupt_read_operations().await.unwrap();
        drop(store);
        let store = TaskStore::open(&path).await.unwrap();
        assert_eq!(
            store.get_system_query(actor, id).await.unwrap().state,
            "unconfirmed"
        );
        assert!(!store.accept_system_query(actor, id, &q).await.unwrap());
    }
}
