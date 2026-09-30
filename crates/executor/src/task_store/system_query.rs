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
            if serde_json::from_str::<SystemQuery>(row.try_get("query_json")?)? != *query {
                return Err(TaskStoreError::RequestConflict);
            }
            return Ok(false);
        }
        sqlx::query("INSERT INTO read_operations (request_id,initiated_by_json,kind,path,state,started_at_unix_ms) VALUES (?,?,?,'','running',?)").bind(id.to_string()).bind(serde_json::to_string(&actor)?).bind(query.kind()).bind(super::operation::now_unix_ms()).execute(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO system_query_results (request_id,query_json,reply_json) VALUES (?,?,?)",
        )
        .bind(id.to_string())
        .bind(serde_json::to_string(query)?)
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
