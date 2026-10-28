use crate::{FileManagerError, decode_path, display_name, encode_path, now_ms};
use pab_protocol::{
    FILE_MANAGER_PAGE_BYTES, FileByteCount, FileManagerFilter, FileManagerSort,
    MAX_FILE_MANAGER_PAGE, ManagedDirectoryPage, ManagedFileEntry, ManagedFileKind, NativeFilePath,
    RequestId,
};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::{Semaphore, mpsc, watch};

const TTL_MS: i64 = 5 * 60 * 1000;
const MAX_SNAPSHOTS: i64 = 32;
const MAX_OWNER_SNAPSHOTS: i64 = 8;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_INDEX_BYTES: i64 = 256 * 1024 * 1024;

/// Owners are supplied by the authenticated adapter, never by an untrusted UI.
#[derive(Clone)]
pub struct DirectoryIndex {
    pool: SqlitePool,
    scans: Arc<Semaphore>,
}
impl DirectoryIndex {
    async fn write_transaction(&self)->Result<sqlx::Transaction<'static,sqlx::Sqlite>,sqlx::Error> {
        crate::begin_write_transaction(&self.pool,"UPDATE fm_snapshots SET touched=touched WHERE 0").await
    }
    pub async fn shutdown(self) {
        self.pool.close().await;
    }
    pub async fn open(database: &Path) -> Result<Self, FileManagerError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(database)
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    // Regenerable directory cache, never the durable task DB.
                    // FULL made each 128-entry batch wait for a remote-disk fsync.
                    .synchronous(SqliteSynchronous::Normal)
                    .pragma("wal_autocheckpoint", "16384")
                    .pragma("journal_size_limit", "16777216")
                    .busy_timeout(Duration::from_secs(10)),
            )
            .await?;
        for statement in [
            "CREATE TABLE IF NOT EXISTS fm_snapshots(id TEXT PRIMARY KEY,owner TEXT NOT NULL,path TEXT NOT NULL,observed INTEGER NOT NULL,touched INTEGER NOT NULL,state TEXT NOT NULL,entries INTEGER NOT NULL DEFAULT 0,bytes INTEGER NOT NULL DEFAULT 0)",
            "CREATE INDEX IF NOT EXISTS fm_snapshot_owner ON fm_snapshots(owner)",
            "CREATE TABLE IF NOT EXISTS fm_entries(snapshot TEXT NOT NULL REFERENCES fm_snapshots(id) ON DELETE CASCADE,seq INTEGER NOT NULL,id TEXT NOT NULL,name TEXT NOT NULL,folder INTEGER NOT NULL,size INTEGER NOT NULL,modified INTEGER NOT NULL,data TEXT NOT NULL,PRIMARY KEY(snapshot,seq),UNIQUE(snapshot,id))",
            // Sorting is materialized once into fm_view_entries. Maintaining
            // three additional sort indexes made every scan write them too.
            "DROP INDEX IF EXISTS fm_entry_name",
            "DROP INDEX IF EXISTS fm_entry_size",
            "DROP INDEX IF EXISTS fm_entry_modified",
            "CREATE TABLE IF NOT EXISTS fm_views(id TEXT PRIMARY KEY,snapshot TEXT NOT NULL REFERENCES fm_snapshots(id) ON DELETE CASCADE,total INTEGER NOT NULL DEFAULT 0,touched INTEGER NOT NULL)",
            "CREATE TABLE IF NOT EXISTS fm_view_entries(view TEXT NOT NULL REFERENCES fm_views(id) ON DELETE CASCADE,rank INTEGER NOT NULL,seq INTEGER NOT NULL,PRIMARY KEY(view,rank))",
        ] {
            sqlx::query(statement).execute(&pool).await?;
        }
        Ok(Self {
            pool,
            scans: Arc::new(Semaphore::new(4)),
        })
    }

    pub async fn scan(
        &self,
        owner: &str,
        path: &NativeFilePath,
        cancel: watch::Receiver<bool>,
    ) -> Result<RequestId, FileManagerError> {
        let _scan_permit = self
            .scans
            .clone()
            .try_acquire_owned()
            .map_err(|_| FileManagerError::Capacity)?;
        if owner.is_empty() || owner.len() > 1024 {
            return Err(FileManagerError::Invalid("invalid directory owner"));
        }
        let native = decode_path(path)?;
        let meta = tokio::fs::symlink_metadata(&native).await?;
        if !meta.is_dir() || meta.file_type().is_symlink() || is_reparse(&meta) {
            return Err(FileManagerError::Invalid(
                "directory must be a real directory",
            ));
        }
        let id = RequestId::new();
        let stamp = now_ms();
        let mut tx = self.write_transaction().await?;
        sqlx::query("DELETE FROM fm_snapshots WHERE touched < ?")
            .bind(stamp - TTL_MS)
            .execute(&mut *tx)
            .await?;
        let (all, owned, bytes): (i64, i64, i64) = sqlx::query_as(
            "SELECT COUNT(*),COALESCE(SUM(owner=?),0),COALESCE(SUM(bytes),0) FROM fm_snapshots",
        )
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
        if all >= MAX_SNAPSHOTS || owned >= MAX_OWNER_SNAPSHOTS || bytes >= MAX_INDEX_BYTES {
            return Err(FileManagerError::Capacity);
        }
        sqlx::query("INSERT INTO fm_snapshots(id,owner,path,observed,touched,state) VALUES(?,?,?,?,?,'scanning')")
            .bind(id.to_string()).bind(owner).bind(serde_json::to_string(path)?)
            .bind(stamp).bind(stamp).execute(&mut *tx).await?;
        tx.commit().await?;
        let (sender, mut receiver) =
            mpsc::channel::<Result<Vec<ManagedFileEntry>, FileManagerError>>(2);
        let worker = tokio::task::spawn_blocking(move || {
            let result = enumerate(id, &native, cancel, &sender);
            if let Err(error) = result {
                let _ = sender.blocking_send(Err(error));
            }
        });
        let result = async {
            let mut count = 0i64;
            while let Some(batch) = receiver.recv().await {
                let batch = batch?;
                let mut tx = self.write_transaction().await?;
                let bytes: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(bytes),0) FROM fm_snapshots").fetch_one(&mut *tx).await?;
                let mut added = 0i64;
                let mut records = Vec::with_capacity(batch.len());
                for entry in batch {
                    let data = serde_json::to_string(&entry)?;
                    added += data.len() as i64;
                    if bytes.saturating_add(added) > MAX_INDEX_BYTES { return Err(FileManagerError::Capacity); }
                    records.push((count,entry,data));
                    count += 1;
                }
                let mut insert = sqlx::QueryBuilder::new("INSERT INTO fm_entries(snapshot,seq,id,name,folder,size,modified,data) ");
                insert.push_values(&records,|mut row,(seq,entry,data)| {
                    row.push_bind(id.to_string()).push_bind(*seq).push_bind(entry.id.to_string())
                        .push_bind(entry.name.to_lowercase()).push_bind(i64::from(entry.kind != ManagedFileKind::Directory))
                        .push_bind(entry.size.map(|n| n.0.min(i64::MAX as u64) as i64).unwrap_or(-1))
                        .push_bind(entry.modified_at_unix_ms.unwrap_or(i64::MIN)).push_bind(data);
                });
                insert.build().execute(&mut *tx).await?;
                let changed = sqlx::query("UPDATE fm_snapshots SET entries=?,bytes=bytes+?,touched=? WHERE id=? AND owner=? AND state='scanning'")
                    .bind(count).bind(added).bind(now_ms()).bind(id.to_string()).bind(owner).execute(&mut *tx).await?.rows_affected();
                if changed != 1 { return Err(FileManagerError::SnapshotUnavailable); }
                tx.commit().await?;
            }
            let changed = sqlx::query("UPDATE fm_snapshots SET state='ready',touched=? WHERE id=? AND owner=? AND state='scanning'")
                .bind(now_ms()).bind(id.to_string()).bind(owner).execute(&self.pool).await?.rows_affected();
            if changed != 1 { return Err(FileManagerError::SnapshotUnavailable); }
            Ok::<_,FileManagerError>(id)
        }.await;
        drop(receiver);
        let joined = worker.await;
        if result.is_err() || joined.is_err() {
            self.close(owner, id).await?;
        }
        joined.map_err(|_| FileManagerError::Interrupted)?;
        result
    }

    pub async fn read(
        &self,
        owner: &str,
        id: RequestId,
        after: Option<&str>,
        limit: u16,
        filter: &FileManagerFilter,
    ) -> Result<ManagedDirectoryPage, FileManagerError> {
        filter.validate().map_err(FileManagerError::Invalid)?;
        if limit == 0 || limit > MAX_FILE_MANAGER_PAGE {
            return Err(FileManagerError::Invalid("invalid directory page size"));
        }
        let fingerprint =
            blake3::hash(format!("{id}:{}", serde_json::to_string(filter)?).as_bytes())
                .to_hex()
                .to_string();
        let offset = match after {
            None => 0i64,
            Some(cursor) => {
                if cursor.len() > 128 {
                    return Err(FileManagerError::Invalid("invalid directory cursor"));
                }
                let (number, key) = cursor
                    .split_once(':')
                    .ok_or(FileManagerError::Invalid("invalid directory cursor"))?;
                if key != fingerprint {
                    return Err(FileManagerError::Invalid(
                        "directory cursor belongs to another query",
                    ));
                }
                let offset = number
                    .parse::<i64>()
                    .map_err(|_| FileManagerError::Invalid("invalid directory offset"))?;
                if offset < 0 || offset > MAX_ENTRIES as i64 {
                    return Err(FileManagerError::Invalid("invalid directory offset"));
                }
                offset
            }
        };
        let mut tx = self.write_transaction().await?;
        let row = sqlx::query("SELECT path,observed FROM fm_snapshots WHERE id=? AND owner=? AND touched>=? AND state='ready'")
            .bind(id.to_string()).bind(owner).bind(now_ms()-TTL_MS).fetch_optional(&mut *tx).await?
            .ok_or(FileManagerError::SnapshotUnavailable)?;
        let path: NativeFilePath = serde_json::from_str(row.try_get("path")?)?;
        let observed_at_unix_ms = row.try_get("observed")?;
        let query = filter.query.to_lowercase();
        let existing: Option<i64> =
            sqlx::query_scalar("SELECT total FROM fm_views WHERE id=? AND snapshot=?")
                .bind(&fingerprint)
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        let total = if let Some(total) = existing {
            total
        } else {
            // Cache the ranked projection once. Re-running COUNT/filter/ORDER
            // for every page was quadratic and repeatedly decoded all JSON.
            let mut used: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(total),0) FROM fm_views")
                .fetch_one(&mut *tx)
                .await?;
            while used > 1_000_000 {
                let oldest = sqlx::query("SELECT id,total FROM fm_views ORDER BY touched LIMIT 1")
                    .fetch_optional(&mut *tx)
                    .await?;
                let Some(oldest) = oldest else {
                    break;
                };
                used -= oldest.try_get::<i64, _>("total")?;
                sqlx::query("DELETE FROM fm_views WHERE id=?")
                    .bind(oldest.try_get::<&str, _>("id")?)
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::query("DELETE FROM fm_views WHERE snapshot=? AND id NOT IN (SELECT id FROM fm_views WHERE snapshot=? ORDER BY touched DESC LIMIT 7)")
                .bind(id.to_string()).bind(id.to_string()).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO fm_views(id,snapshot,touched) VALUES(?,?,?)")
                .bind(&fingerprint)
                .bind(id.to_string())
                .bind(now_ms())
                .execute(&mut *tx)
                .await?;
            let order = match filter.sort {
                FileManagerSort::Name => "name",
                FileManagerSort::Size => "size",
                FileManagerSort::Modified => "modified",
            };
            let direction = if filter.descending { "DESC" } else { "ASC" };
            let sql = format!(
                "INSERT INTO fm_view_entries(view,rank,seq) SELECT ?,ROW_NUMBER() OVER(ORDER BY folder ASC,{order} {direction},name ASC,seq ASC)-1,seq FROM fm_entries WHERE snapshot=? AND (? OR json_extract(data,'$.hidden')=0) AND instr(name,?)>0"
            );
            let total = sqlx::query(&sql)
                .bind(&fingerprint)
                .bind(id.to_string())
                .bind(filter.show_hidden)
                .bind(&query)
                .execute(&mut *tx)
                .await?
                .rows_affected() as i64;
            sqlx::query("UPDATE fm_views SET total=? WHERE id=?")
                .bind(total)
                .bind(&fingerprint)
                .execute(&mut *tx)
                .await?;
            total
        };
        let rows=sqlx::query_scalar::<_,String>("SELECT e.data FROM fm_view_entries v JOIN fm_entries e ON e.snapshot=? AND e.seq=v.seq WHERE v.view=? AND v.rank>=? ORDER BY v.rank LIMIT ?")
            .bind(id.to_string()).bind(&fingerprint).bind(offset).bind(i64::from(limit)).fetch_all(&mut *tx).await?;
        sqlx::query("UPDATE fm_views SET touched=? WHERE id=?")
            .bind(now_ms())
            .bind(&fingerprint)
            .execute(&mut *tx)
            .await?;
        let mut page = ManagedDirectoryPage {
            snapshot_id: id,
            path,
            entries: Vec::new(),
            next_after: None,
            total_entries: FileByteCount(total as u64),
            observed_at_unix_ms,
            complete: true,
        };
        let mut bytes = serde_json::to_vec(&page)?.len() + 256;
        for data in rows {
            if bytes + data.len() + 1 > FILE_MANAGER_PAGE_BYTES && !page.entries.is_empty() {
                break;
            }
            bytes += data.len() + 1;
            if bytes > FILE_MANAGER_PAGE_BYTES {
                return Err(FileManagerError::Invalid(
                    "directory entry exceeds wire limit",
                ));
            }
            page.entries.push(serde_json::from_str(&data)?);
        }
        let next = offset + page.entries.len() as i64;
        if next < total {
            page.next_after = Some(format!("{next}:{fingerprint}"));
        }
        sqlx::query("UPDATE fm_snapshots SET touched=? WHERE id=? AND owner=?")
            .bind(now_ms())
            .bind(id.to_string())
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(page)
    }

    pub async fn entry(
        &self,
        owner: &str,
        snapshot: RequestId,
        entry: RequestId,
    ) -> Result<ManagedFileEntry, FileManagerError> {
        let data: Option<String> = sqlx::query_scalar("SELECT e.data FROM fm_entries e JOIN fm_snapshots s ON s.id=e.snapshot WHERE e.snapshot=? AND e.id=? AND s.owner=? AND s.state='ready' AND s.touched>=?")
            .bind(snapshot.to_string()).bind(entry.to_string()).bind(owner).bind(now_ms()-TTL_MS).fetch_optional(&self.pool).await?;
        Ok(serde_json::from_str(
            &data.ok_or(FileManagerError::SnapshotUnavailable)?,
        )?)
    }

    pub async fn close(&self, owner: &str, id: RequestId) -> Result<(), FileManagerError> {
        sqlx::query("DELETE FROM fm_snapshots WHERE id=? AND owner=?")
            .bind(id.to_string())
            .bind(owner)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn release_owner(&self, owner: &str) -> Result<(), FileManagerError> {
        sqlx::query("DELETE FROM fm_snapshots WHERE owner=?")
            .bind(owner)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn enumerate(
    snapshot: RequestId,
    path: &Path,
    cancel: watch::Receiver<bool>,
    sender: &mpsc::Sender<Result<Vec<ManagedFileEntry>, FileManagerError>>,
) -> Result<(), FileManagerError> {
    let reader = std::fs::read_dir(path)?;
    let mut batch = Vec::with_capacity(128);
    for (count, item) in reader.enumerate() {
        if *cancel.borrow() || sender.is_closed() {
            return Err(FileManagerError::Interrupted);
        }
        if count >= MAX_ENTRIES {
            return Err(FileManagerError::Capacity);
        }
        let item = item?;
        let native = item.path();
        let name = display_name(&item.file_name());
        let encoded = encode_path(&native);
        encoded.validate().map_err(FileManagerError::Invalid)?;
        // DirEntry metadata does not follow links. On Windows the directory
        // enumeration already contains it; avoid a separate open per child.
        let meta = item.metadata();
        // IDs are snapshot-local references, never authorization. Monotonic
        // suffixes avoid random B-tree page writes on slow remote storage.
        let id = (snapshot.as_uuid().as_u128() & !(u64::MAX as u128)) | (count as u128);
        let mut entry = ManagedFileEntry {
            id: RequestId::from_u128(id),
            name: name.clone(),
            path: encoded,
            kind: ManagedFileKind::Other,
            size: None,
            modified_at_unix_ms: None,
            hidden: name.starts_with('.'),
            readonly: false,
            unix_mode: None,
            error: None,
        };
        match meta {
            Ok(meta) => {
                entry.kind = if meta.file_type().is_symlink() || is_reparse(&meta) {
                    ManagedFileKind::Symlink
                } else if meta.is_dir() {
                    ManagedFileKind::Directory
                } else if meta.is_file() {
                    ManagedFileKind::File
                } else {
                    ManagedFileKind::Other
                };
                entry.size =
                    (entry.kind == ManagedFileKind::File).then_some(FileByteCount(meta.len()));
                entry.modified_at_unix_ms = crate::modified_at_unix_ms(&meta);
                entry.readonly = meta.permissions().readonly();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    entry.unix_mode = Some(meta.mode() & 0o777);
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    entry.hidden |= meta.file_attributes() & 2 != 0;
                }
            }
            Err(error) => entry.error = Some(error.kind().to_string()),
        }
        batch.push(entry);
        if batch.len() == 128 {
            sender
                .blocking_send(Ok(std::mem::take(&mut batch)))
                .map_err(|_| FileManagerError::Interrupted)?;
        }
    }
    if *cancel.borrow() || sender.is_closed() {
        return Err(FileManagerError::Interrupted);
    }
    if !batch.is_empty() {
        sender
            .blocking_send(Ok(batch))
            .map_err(|_| FileManagerError::Interrupted)?;
    }
    Ok(())
}
fn is_reparse(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = meta;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn indexed_pages_are_sorted_filtered_and_scoped_without_rescanning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("files");
        tokio::fs::create_dir(&root).await.unwrap();
        for (name, size) in [("c.txt", 3), ("a.txt", 1), ("b.txt", 2), (".hidden", 0)] {
            tokio::fs::write(root.join(name), vec![1; size])
                .await
                .unwrap();
        }
        tokio::fs::create_dir(root.join("folder")).await.unwrap();
        let index = DirectoryIndex::open(&dir.path().join("index.db"))
            .await
            .unwrap();
        let (_stop, cancel) = watch::channel(false);
        let id = index
            .scan("account-a", &encode_path(&root), cancel)
            .await
            .unwrap();
        let filter = FileManagerFilter::default();
        let first = index.read("account-a", id, None, 2, &filter).await.unwrap();
        assert_eq!(first.total_entries.0, 4);
        assert_eq!(
            first
                .entries
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["folder", "a.txt"]
        );
        assert!(matches!(
            index.read("account-b", id, None, 2, &filter).await,
            Err(FileManagerError::SnapshotUnavailable)
        ));
        // Removing the source demonstrates that later pages use the index.
        tokio::fs::remove_dir_all(&root).await.unwrap();
        let second = index
            .read("account-a", id, first.next_after.as_deref(), 2, &filter)
            .await
            .unwrap();
        assert_eq!(
            second
                .entries
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["b.txt", "c.txt"]
        );
        assert!(second.next_after.is_none());
        let sorted = FileManagerFilter {
            sort: FileManagerSort::Size,
            descending: true,
            query: ".txt".into(),
            ..Default::default()
        };
        assert!(
            index
                .read("account-a", id, first.next_after.as_deref(), 2, &sorted)
                .await
                .is_err()
        );
        let page = index.read("account-a", id, None, 1, &sorted).await.unwrap();
        assert_eq!(page.entries[0].name, "c.txt");
        assert_eq!(page.total_entries.0, 3);
        let entry = &page.entries[0];
        assert_eq!(
            index.entry("account-a", id, entry.id).await.unwrap(),
            *entry
        );
        assert!(index.entry("account-b", id, entry.id).await.is_err());
        index.close("account-b", id).await.unwrap();
        assert!(index.read("account-a", id, None, 2, &filter).await.is_ok());
        index.release_owner("account-a").await.unwrap();
        assert!(index.read("account-a", id, None, 2, &filter).await.is_err());
    }
    #[tokio::test]
    async fn interrupted_scans_leave_no_snapshot_or_rows() {
        let dir = tempfile::tempdir().unwrap();
        let index = DirectoryIndex::open(&dir.path().join("index.db"))
            .await
            .unwrap();
        let (_stop, cancel) = watch::channel(true);
        assert!(matches!(
            index.scan("owner", &encode_path(dir.path()), cancel).await,
            Err(FileManagerError::Interrupted)
        ));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fm_snapshots")
            .fetch_one(&index.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
