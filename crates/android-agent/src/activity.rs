//! Privacy-limited local activity projection through the same SQLite library as the writers.
//! Android's framework SQLite must never open these live WAL databases: its independent
//! process-local lock bookkeeping can release locks owned by bundled SQLite connections.

use serde_json::{Value, json};
use sqlx::{Connection, Row, sqlite::SqliteConnectOptions};
use std::{path::Path, time::Duration};

pub(crate) async fn read(directory: &Path) -> Value {
    let sources = [
        (
            "mobile.sqlite",
            "mobile_operations",
            "reply",
            "interface",
            true,
        ),
        (
            "mobile.system.sqlite",
            "system_operations",
            "reply",
            "system",
            true,
        ),
        (
            "mobile.files.sqlite",
            "file_operations",
            "reply",
            "file",
            false,
        ),
        (
            "mobile.documents.sqlite",
            "document_operations",
            "reply",
            "document",
            true,
        ),
        (
            "mobile.execution.sqlite",
            "command_tasks",
            "snapshot",
            "execution",
            true,
        ),
    ];
    let mut records = Vec::new();
    let mut unavailable = false;
    for (name, table, column, category, rowid) in sources {
        let path = directory.join(name);
        if !path.is_file() {
            continue;
        }
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .read_only(true)
            .create_if_missing(false)
            .busy_timeout(Duration::from_millis(250));
        let Ok(mut connection) = sqlx::SqliteConnection::connect_with(&options).await else {
            unavailable = true;
            continue;
        };
        let json = column;
        // All identifiers above are constants. Only an allowlist of scalar metadata
        // crosses JNI; file/notification/command contents never leave this projection.
        let sql = format!(
            "SELECT id, json_extract({json},'$.kind') AS kind, \
             json_extract({json},'$.state') AS state, \
             CASE WHEN json_type({json},'$.error')='text' THEN json_extract({json},'$.error') \
             ELSE json_extract({json},'$.error.code') END AS error, \
             coalesce(json_extract({json},'$.data.snapshot.ui.outcome'), \
             json_extract({json},'$.data.snapshot.outcome')) AS outcome, \
             coalesce(json_extract({json},'$.sampled_at_unix_ms'), \
             json_extract({json},'$.sampled_from_unix_ms'), \
             json_extract({json},'$.finished_at_unix_ms'), \
             json_extract({json},'$.created_at_unix_ms'), \
             json_extract({json},'$.progress.updated_at_unix_ms')) AS timestamp \
             FROM {table} WHERE json_valid({json}) ORDER BY timestamp DESC, {} DESC LIMIT 100",
            if rowid { "rowid" } else { "id" },
        );
        match sqlx::query(&sql).fetch_all(&mut connection).await {
            Ok(rows) => {
                for row in rows {
                    let Ok(id) = row.try_get::<String, _>("id") else {
                        unavailable = true;
                        continue;
                    };
                    records.push(json!({
                    "id":format!("{name}:{id}"), "source":category,
                    "kind":row.try_get::<Option<String>, _>("kind").ok().flatten().unwrap_or_default(),
                    "state":row.try_get::<Option<String>, _>("state").ok().flatten().unwrap_or_default(),
                    "error":row.try_get::<Option<String>, _>("error").ok().flatten().unwrap_or_default(),
                    "outcome":row.try_get::<Option<String>, _>("outcome").ok().flatten().unwrap_or_default(),
                    "timestamp":row.try_get::<Option<i64>, _>("timestamp").ok().flatten().filter(|time| *time>0),
                }));
                }
            }
            Err(_) => unavailable = true,
        }
        if connection.close().await.is_err() {
            unavailable = true;
        }
    }
    records.sort_by_key(|record| std::cmp::Reverse(record["timestamp"].as_i64().unwrap_or(0)));
    records.truncate(100);
    json!({"records":records,"unavailable":unavailable})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn projects_only_metadata_and_does_not_create_or_modify_databases() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(read(directory.path()).await["records"], json!([]));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        let path = directory.path().join("mobile.system.sqlite");
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let mut writer = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE system_operations(id TEXT, reply TEXT)")
            .execute(&mut writer)
            .await
            .unwrap();
        let receipt = json!({"kind":"notification_open","state":"completed","sampled_at_unix_ms":123,
            "data":{"snapshot":{"outcome":"dispatched","entries":[{"text":"private-notification"}]}},
            "password":"private-password","path":"private-path"});
        sqlx::query("INSERT INTO system_operations VALUES('one',?)")
            .bind(receipt.to_string())
            .execute(&mut writer)
            .await
            .unwrap();
        let value = read(directory.path()).await;
        assert_eq!(value["records"][0]["timestamp"], 123);
        assert_eq!(value["records"][0]["outcome"], "dispatched");
        assert!(!value.to_string().contains("private-"));
        let stored: String = sqlx::query_scalar("SELECT reply FROM system_operations")
            .fetch_one(&mut writer)
            .await
            .unwrap();
        assert_eq!(stored, receipt.to_string());
        writer.close().await.unwrap();
    }

    #[tokio::test]
    async fn reads_live_wal_during_writes_and_truncating_checkpoints() {
        let directory = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(directory.path().join("mobile.files.sqlite"))
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let mut writer = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE file_operations(id TEXT PRIMARY KEY,reply TEXT) WITHOUT ROWID")
            .execute(&mut writer)
            .await
            .unwrap();
        let path = directory.path().to_owned();
        let reader = tokio::spawn(async move {
            for _ in 0..80 {
                let value = read(&path).await;
                assert!(!value["unavailable"].as_bool().unwrap());
            }
        });
        for index in 0..160 {
            let receipt = json!({"kind":"file_read","state":"completed","progress":{"updated_at_unix_ms":index+1}});
            sqlx::query("INSERT INTO file_operations VALUES(?,?)")
                .bind(index.to_string())
                .bind(receipt.to_string())
                .execute(&mut writer)
                .await
                .unwrap();
            sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                .execute(&mut writer)
                .await
                .unwrap();
        }
        reader.await.unwrap();
        let result = read(directory.path()).await;
        assert_eq!(result["records"].as_array().unwrap().len(), 100);
        assert_eq!(result["records"][0]["timestamp"], 160);
        writer.close().await.unwrap();
    }
}
