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
            false,
        ),
        (
            "mobile.system.sqlite",
            "system_operations",
            "reply",
            "system",
            false,
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
            false,
        ),
        (
            "mobile.execution.sqlite",
            "command_tasks",
            "snapshot",
            "execution",
            false,
        ),
        (
            "mobile.sqlite",
            "mobile_operation_archive",
            "",
            "interface",
            true,
        ),
    ];
    let mut records = Vec::new();
    let mut unavailable = false;
    for (name, table, column, category, archived) in sources {
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
        let sql = if archived {
            // Older journals may predate archival. The compact archive preserves
            // state but has no kind or timestamp; show it in undated history.
            let exists = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='mobile_operation_archive'",
            ).fetch_one(&mut connection).await;
            match exists {
                Ok(0) => {
                    if connection.close().await.is_err() {
                        unavailable = true;
                    }
                    continue;
                }
                Err(_) => unavailable = true,
                _ => {}
            }
            "SELECT owner,id,'' AS kind,state,error,'' AS outcome,NULL AS timestamp FROM mobile_operation_archive ORDER BY owner,id".to_owned()
        } else {
            format!(
                "SELECT owner, id, json_extract({json},'$.kind') AS kind, \
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
             FROM {table} WHERE json_valid({json}) ORDER BY timestamp DESC, owner, id"
            )
        };
        match sqlx::query(&sql).fetch_all(&mut connection).await {
            Ok(rows) => {
                for row in rows {
                    let Ok(id) = row.try_get::<String, _>("id") else {
                        unavailable = true;
                        continue;
                    };
                    let Ok(owner) = row.try_get::<String, _>("owner") else {
                        unavailable = true;
                        continue;
                    };
                    // Request IDs are only unique within an owner. Keep owners
                    // opaque while giving Compose a stable, globally unique key.
                    let key = blake3::hash(format!("{}:{owner}{id}", owner.len()).as_bytes());
                    records.push(json!({
                    "id":format!("{name}:{key}"), "source":category,
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
    // Compaction can move a receipt between the live and archive queries. Keep
    // the dated live projection if both were observed during that transition.
    let mut seen = std::collections::HashSet::new();
    records.retain(|record| seen.insert(record["id"].as_str().unwrap().to_owned()));
    json!({"records":records,"unavailable":unavailable})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_all_sources_archives_and_owners_without_a_display_cap() {
        let directory = tempfile::tempdir().unwrap();
        for (file, table, column) in [
            ("mobile.sqlite", "mobile_operations", "reply"),
            ("mobile.system.sqlite", "system_operations", "reply"),
            ("mobile.files.sqlite", "file_operations", "reply"),
            ("mobile.documents.sqlite", "document_operations", "reply"),
            ("mobile.execution.sqlite", "command_tasks", "snapshot"),
        ] {
            let options = SqliteConnectOptions::new()
                .filename(directory.path().join(file))
                .create_if_missing(true);
            let mut writer = sqlx::SqliteConnection::connect_with(&options)
                .await
                .unwrap();
            sqlx::query(&format!(
                "CREATE TABLE {table}(owner TEXT,id TEXT,{column} TEXT,PRIMARY KEY(owner,id))"
            ))
            .execute(&mut writer)
            .await
            .unwrap();
            for index in 0..137 {
                let receipt = json!({"state":if index%2==0 {"completed"} else {"failed"},
                    "sampled_at_unix_ms":index+1,"data":{"text":"private-payload"}});
                sqlx::query(&format!("INSERT INTO {table} VALUES('private-owner',?,?)"))
                    .bind(index.to_string())
                    .bind(receipt.to_string())
                    .execute(&mut writer)
                    .await
                    .unwrap();
            }
            if table == "mobile_operations" {
                // An older database without the archive table remains readable.
                let value = read(directory.path()).await;
                assert_eq!(value["records"].as_array().unwrap().len(), 137);
                assert_eq!(value["unavailable"], false);
                sqlx::query("CREATE TABLE mobile_operation_archive(owner TEXT,id TEXT,state TEXT,error TEXT,PRIMARY KEY(owner,id)) WITHOUT ROWID")
                    .execute(&mut writer).await.unwrap();
                for index in 0..132 {
                    sqlx::query("INSERT INTO mobile_operation_archive VALUES('private-owner',?,'completed',NULL)")
                        .bind(format!("archived-{index}")).execute(&mut writer).await.unwrap();
                }
                // Same ID owned by different clients must produce distinct keys.
                sqlx::query("INSERT INTO mobile_operation_archive VALUES('other-owner','0','failed',NULL),('private-owner','0','completed',NULL)")
                    .execute(&mut writer).await.unwrap();
            }
            writer.close().await.unwrap();
        }
        let value = read(directory.path()).await;
        assert_eq!(value["unavailable"], false);
        let records = value["records"].as_array().unwrap();
        assert_eq!(records.len(), 137 * 5 + 132 + 1);
        assert_eq!(
            records.iter().filter(|r| r["timestamp"].is_null()).count(),
            133
        );
        assert_eq!(records.first().unwrap()["timestamp"], 137);
        assert!(records.last().unwrap()["timestamp"].is_null());
        for source in ["interface", "system", "file", "document", "execution"] {
            assert_eq!(
                records
                    .iter()
                    .filter(|r| r["source"] == source && r["timestamp"] == 1)
                    .count(),
                1
            );
        }
        let keys: std::collections::HashSet<_> =
            records.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(keys.len(), records.len());
        assert!(!value.to_string().contains("private-"));
        assert!(!value.to_string().contains("other-owner"));
        assert_eq!(value, read(directory.path()).await);
    }

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
        sqlx::query("CREATE TABLE system_operations(owner TEXT, id TEXT, reply TEXT)")
            .execute(&mut writer)
            .await
            .unwrap();
        let receipt = json!({"kind":"notification_open","state":"completed","sampled_at_unix_ms":123,
            "data":{"snapshot":{"outcome":"dispatched","entries":[{"text":"private-notification"}]}},
            "password":"private-password","path":"private-path"});
        sqlx::query("INSERT INTO system_operations VALUES('private-owner','one',?)")
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
        sqlx::query("CREATE TABLE file_operations(owner TEXT,id TEXT,reply TEXT,PRIMARY KEY(owner,id)) WITHOUT ROWID")
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
            sqlx::query("INSERT INTO file_operations VALUES('owner',?,?)")
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
        assert_eq!(result["records"].as_array().unwrap().len(), 160);
        assert_eq!(result["records"][0]["timestamp"], 160);
        writer.close().await.unwrap();
    }
}
