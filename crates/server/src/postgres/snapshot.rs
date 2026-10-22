use super::PostgresStore;

impl PostgresStore {
    /// Complete BEGIN even if the HTTP caller disconnects while PostgreSQL is
    /// acknowledging it. The returned transaction owns its pool connection;
    /// dropping an unclaimed result then queues a normal, tracked rollback.
    /// No query/body or application mutation runs in this detached task.
    pub async fn begin_snapshot(
        &self,
        read_only: bool,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
        let pool = self.pool.clone();
        tokio::spawn(async move {
            pool.begin_with(if read_only {
                "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY"
            } else {
                "BEGIN ISOLATION LEVEL REPEATABLE READ"
            })
            .await
        })
        .await
        .map_err(|_| sqlx::Error::Protocol("snapshot initialization task failed".into()))?
    }
}
