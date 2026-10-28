//! Cancellation-safe acquisition of SQLite's single writer.
use sqlx::{Sqlite,SqlitePool,Transaction};

/// Begin a normal RAII transaction, then take the write lock before reading.
/// `lock_statement` is a static no-op UPDATE of an existing application table.
///
/// sqlx 0.8's custom BEGIN has another await after SQLite acknowledges BEGIN
/// and before constructing Transaction. Cancelling there can return a live
/// transaction to the pool. Here every await after BEGIN owns a Transaction,
/// so cancellation queues its rollback before the connection can be reused.
#[doc(hidden)]
pub async fn begin_write_transaction(pool:&SqlitePool,lock_statement:&'static str)->Result<Transaction<'static,Sqlite>,sqlx::Error> {
    let mut transaction=pool.begin().await?;
    sqlx::query(lock_statement).execute(&mut *transaction).await?;
    Ok(transaction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use sqlx::sqlite::{SqliteConnectOptions,SqlitePoolOptions};

    #[tokio::test]
    async fn cancelling_a_waiting_writer_never_leaks_a_transaction_or_its_changes() {
        let temp=tempfile::tempdir().unwrap();let path=temp.path().join("cancel.sqlite3");
        let options=SqliteConnectOptions::new().filename(path).create_if_missing(true).busy_timeout(Duration::from_secs(3));
        let pool=SqlitePoolOptions::new().max_connections(1).connect_with(options.clone()).await.unwrap();
        let other=SqlitePoolOptions::new().max_connections(1).connect_with(options).await.unwrap();
        sqlx::query("CREATE TABLE state(value INTEGER NOT NULL)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO state VALUES(0)").execute(&pool).await.unwrap();
        const LOCK:&str="UPDATE state SET value=value WHERE 0";
        for _ in 0..100 {
            let blocker=begin_write_transaction(&other,LOCK).await.unwrap();
            let contender=pool.clone();let (begun,waiting)=tokio::sync::oneshot::channel();
            let task=tokio::spawn(async move {
                let _=begun.send(());
                let mut transaction=begin_write_transaction(&contender,LOCK).await.unwrap();
                sqlx::query("UPDATE state SET value=1").execute(&mut *transaction).await.unwrap();
                transaction.commit().await.unwrap();
            });
            waiting.await.unwrap();tokio::time::sleep(Duration::from_millis(1)).await;
            assert!(!task.is_finished(),"no-op UPDATE must reserve the SQLite writer");
            task.abort();assert!(task.await.unwrap_err().is_cancelled());blocker.rollback().await.unwrap();
            let mut next=tokio::time::timeout(Duration::from_secs(3),begin_write_transaction(&pool,LOCK)).await.unwrap().unwrap();
            let value:i64=sqlx::query_scalar("SELECT value FROM state").fetch_one(&mut *next).await.unwrap();assert_eq!(value,0,"cancelled work must never commit");next.commit().await.unwrap();
            // A custom top-level BEGIN rejects any leaked transaction depth.
            // This probe is deliberately never cancelled.
            pool.begin_with("BEGIN IMMEDIATE").await.unwrap().rollback().await.unwrap();
        }
    }
}
