//! Existing document protocol over immutable, authorized snapshots. Parser code runs
//! in the APK's disposable worker process, never in the connection process.
use crate::{filesystem::FileService, storage::StorageBridge};
use pab_document::{Document, query};
use pab_protocol::*;
use serde_json::{Value, json};
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Notify, Semaphore};

const INPUT_LIMIT: u64 = 32 * 1024 * 1024;
const RESULT_LIMIT: u64 = 16 * 1024 * 1024;
const CACHE_LIMIT: usize = 32 * 1024 * 1024;
const LIFETIME: Duration = Duration::from_secs(900);
type Key = (String, RequestId);

struct Snapshot {
    directory: PathBuf,
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        // Only fixed filenames created by this module. Never recursively remove a tree.
        for name in ["input", "result.json", "preview.png"] {
            let _ = std::fs::remove_file(self.directory.join(name));
        }
        let _ = std::fs::remove_dir(&self.directory);
    }
}
struct Entry {
    owner: String,
    source: String,
    snapshot: Arc<Snapshot>,
    document: Arc<Document>,
    created: Instant,
    previews: Vec<(String, String)>,
}
pub(crate) struct DocumentService {
    pub sdk: Arc<StorageBridge>,
    files: Arc<FileService>,
    pool: SqlitePool,
    key: [u8; 32],
    root: PathBuf,
    acceptance: Mutex<()>,
    entries: Mutex<HashMap<RequestId, Entry>>,
    replies: Mutex<VecDeque<(Key, DocumentReply)>>,
    jobs: Mutex<HashMap<Key, Arc<AtomicBool>>>,
    slots: Arc<Semaphore>,
    worker: Arc<Semaphore>,
    changed: Notify,
}

impl DocumentService {
    pub async fn open(
        path: &Path,
        key: [u8; 32],
        files: Arc<FileService>,
    ) -> Result<Arc<Self>, &'static str> {
        let root = path.with_file_name("mobile.document-cache");
        std::fs::create_dir_all(&root).map_err(|_| "document_cache_unavailable")?;
        if root
            .symlink_metadata()
            .map_err(|_| "document_cache_unavailable")?
            .file_type()
            .is_symlink()
        {
            return Err("document_cache_unavailable");
        }
        // Recover only owned, fixed-name scratch files after process death.
        for child in std::fs::read_dir(&root)
            .map_err(|_| "document_cache_unavailable")?
            .flatten()
        {
            if child.file_type().is_ok_and(|t| t.is_dir())
                && child
                    .file_name()
                    .to_str()
                    .is_some_and(|n| uuid::Uuid::parse_str(n).is_ok())
            {
                drop(Snapshot {
                    directory: child.path(),
                });
            }
        }
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Full);
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .map_err(|_| "document_journal_unavailable")?;
        sqlx::query("CREATE TABLE IF NOT EXISTS document_operations(owner TEXT NOT NULL,id TEXT NOT NULL,digest TEXT NOT NULL,reply TEXT NOT NULL,PRIMARY KEY(owner,id))")
            .execute(&pool).await.map_err(|_| "document_journal_unavailable")?;
        for row in sqlx::query("SELECT owner,id,reply FROM document_operations")
            .fetch_all(&pool)
            .await
            .map_err(|_| "document_journal_unavailable")?
        {
            let mut reply: DocumentReply =
                serde_json::from_str(row.get("reply")).map_err(|_| "document_journal_invalid")?;
            if matches!(reply.state.as_str(), "running" | "cancel_requested") {
                reply = failed(reply, "process_restarted_original_not_replayed");
                sqlx::query("UPDATE document_operations SET reply=? WHERE owner=? AND id=?")
                    .bind(serde_json::to_string(&reply).unwrap())
                    .bind(row.get::<String, _>("owner"))
                    .bind(row.get::<String, _>("id"))
                    .execute(&pool)
                    .await
                    .map_err(|_| "document_journal_unavailable")?;
            }
        }
        sqlx::query("CREATE TABLE IF NOT EXISTS document_previews(path TEXT PRIMARY KEY,hash TEXT NOT NULL)")
            .execute(&pool).await.map_err(|_| "document_journal_unavailable")?;
        let service = Arc::new(Self {
            sdk: StorageBridge::for_domain("document"),
            files,
            pool,
            key,
            root,
            acceptance: Mutex::new(()),
            entries: Mutex::new(HashMap::new()),
            replies: Mutex::new(VecDeque::new()),
            jobs: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(4)),
            worker: Arc::new(Semaphore::new(1)),
            changed: Notify::new(),
        });
        let weak = Arc::downgrade(&service);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let Some(service) = weak.upgrade() else { break };
                service.expire().await;
                service.cleanup_orphan_previews().await;
            }
        });
        Ok(service)
    }

    pub async fn request(self: &Arc<Self>, owner: &str, request: DocumentRequest) -> DocumentReply {
        let pending = DocumentReply::pending(&request);
        if request.validate().is_err() {
            return failed(pending, "invalid_request");
        }
        // User-context selections are not silently substituted by the service UID.
        if let DocumentAction::Open { execution, .. } = &request.operation
            && !matches!(execution, ExecutionSelection::Service {})
        {
            return failed(pending, "android_documents_require_service_execution");
        }
        let digest =
            blake3::keyed_hash(&self.key, &serde_json::to_vec(&request.operation).unwrap())
                .to_hex()
                .to_string();
        let permit;
        let key = (owner.to_owned(), request.request_id);
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let _guard = self.acceptance.lock().await;
            match sqlx::query_scalar::<_, String>(
                "SELECT digest FROM document_operations WHERE owner=? AND id=?",
            )
            .bind(owner)
            .bind(request.request_id.to_string())
            .fetch_optional(&self.pool)
            .await
            {
                Ok(Some(existing)) => {
                    return if existing == digest {
                        drop(_guard);
                        self.authorized_lookup(owner, request.request_id).await
                    } else {
                        failed(pending, "request_conflict")
                    };
                }
                Err(_) => return failed(pending, "document_journal_unavailable"),
                _ => {}
            }
            permit = match self.slots.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => return failed(pending, "document_busy"),
            };
            if !matches!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM document_operations").fetch_one(&self.pool).await, Ok(n) if n < 10000)
            {
                return failed(pending, "document_journal_capacity_or_unavailable");
            }
            if sqlx::query("INSERT INTO document_operations(owner,id,digest,reply) VALUES(?,?,?,?)")
                .bind(owner)
                .bind(request.request_id.to_string())
                .bind(digest)
                .bind(serde_json::to_string(&pending).unwrap())
                .execute(&self.pool)
                .await
                .is_err()
            {
                return failed(pending, "document_acceptance_failed");
            }
            self.jobs
                .lock()
                .await
                .insert(key.clone(), cancelled.clone());
        }
        let service = self.clone();
        let lookup_key = key.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let mut reply = service.execute(&key.0, &request, &cancelled).await;
            if cancelled.load(Ordering::Acquire)
                && matches!(request.operation, DocumentAction::Open { .. })
            {
                service.remove(&key.0, request.request_id).await;
                reply.state = "cancelled".into();
                reply.document_ref = None;
                reply.data = None;
                reply.error = None;
            }
            if serde_json::to_vec(&reply).is_ok_and(|b| b.len() > 30 * 1024) {
                if reply.kind == "document_open" {
                    service.remove(&key.0, request.request_id).await;
                }
                reply = failed(reply, "document_reply_limit");
            }
            let mut saved = reply.clone();
            saved.data = None;
            if sqlx::query("UPDATE document_operations SET reply=? WHERE owner=? AND id=?")
                .bind(serde_json::to_string(&saved).unwrap())
                .bind(&key.0)
                .bind(key.1.to_string())
                .execute(&service.pool)
                .await
                .is_err()
            {
                reply = failed(
                    reply,
                    "document_outcome_persistence_failed_original_not_replayed",
                );
            }
            let mut replies = service.replies.lock().await;
            if replies.len() >= 64 {
                replies.pop_front();
            }
            replies.push_back((key.clone(), reply));
            drop(replies);
            service.jobs.lock().await.remove(&key);
            service.changed.notify_waiters();
        });
        // Open is explicitly asynchronous; other queries may also outlive the stream.
        let wait = if pending.kind == "document_open" {
            0
        } else {
            4
        };
        let _ = tokio::time::timeout(Duration::from_secs(wait), async {
            loop {
                let changed = self.changed.notified();
                if !self.jobs.lock().await.contains_key(&lookup_key) {
                    break;
                }
                changed.await;
            }
        })
        .await;
        self.authorized_lookup(owner, pending.request_id).await
    }

    async fn lookup(&self, owner: &str, id: RequestId) -> DocumentReply {
        if let Some((_, reply)) = self
            .replies
            .lock()
            .await
            .iter()
            .find(|(k, _)| k == &(owner.to_owned(), id))
        {
            return reply.clone();
        }
        let base = DocumentReply {
            request_id: id,
            kind: "document_open".into(),
            state: "failed".into(),
            document_ref: None,
            data: None,
            error: None,
        };
        match sqlx::query_scalar::<_, String>(
            "SELECT reply FROM document_operations WHERE owner=? AND id=?",
        )
        .bind(owner)
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        {
            Ok(Some(raw)) => match serde_json::from_str::<DocumentReply>(&raw) {
                Ok(mut reply) if reply.state == "completed" => {
                    // Recheck after the DB await: publication into the memory cache
                    // may have happened since the first lookup above.
                    if let Some((_, cached)) = self
                        .replies
                        .lock()
                        .await
                        .iter()
                        .find(|(key, _)| key == &(owner.to_owned(), id))
                    {
                        return cached.clone();
                    }
                    // Durable completion precedes transient result publication.
                    if self.jobs.lock().await.contains_key(&(owner.to_owned(), id)) {
                        reply.state = "running".into();
                        reply.data = None;
                        reply
                    } else {
                        failed(reply, "result_expired_original_not_replayed")
                    }
                }
                Ok(reply) => reply,
                Err(_) => failed(base, "document_journal_invalid"),
            },
            Ok(None) => failed(base, "document_expired_or_not_found"),
            Err(_) => failed(base, "document_journal_unavailable"),
        }
    }

    pub async fn authorized_lookup(&self, owner: &str, id: RequestId) -> DocumentReply {
        let reply = self.lookup(owner, id).await;
        if reply.state != "completed" {
            return reply;
        }
        if let Some(reference) = reply.document_ref {
            let source = self
                .entries
                .lock()
                .await
                .get(&reference)
                .filter(|e| e.owner == owner && e.created.elapsed() < LIFETIME)
                .map(|e| e.source.clone());
            let Some(source) = source else {
                return failed(reply, "document_expired_or_closed");
            };
            if let Err(code) = self
                .sdk_call(json!({"action":"authorize","path":source}))
                .await
            {
                return failed(reply, &code);
            }
        }
        reply
    }

    pub async fn cancel(&self, owner: &str, id: RequestId) -> DocumentReply {
        if let Some(cancelled) = self.jobs.lock().await.get(&(owner.to_owned(), id)) {
            cancelled.store(true, Ordering::Release);
        }
        let mut reply = self.authorized_lookup(owner, id).await;
        if reply.state == "running" {
            reply.state = "cancel_requested".into();
        }
        reply
    }

    async fn execute(
        &self,
        owner: &str,
        request: &DocumentRequest,
        cancelled: &AtomicBool,
    ) -> DocumentReply {
        let pending = DocumentReply::pending(request);
        match self.perform(owner, request, cancelled).await {
            Ok(reply) => reply,
            Err(code) => failed(pending, &code),
        }
    }
    async fn perform(
        &self,
        owner: &str,
        request: &DocumentRequest,
        cancelled: &AtomicBool,
    ) -> Result<DocumentReply, String> {
        let mut reply = DocumentReply::pending(request);
        if cancelled.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        match &request.operation {
            DocumentAction::Open { path, ocr_mode, .. } => {
                let _worker = self
                    .worker
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| "document_worker_busy")?;
                self.expire().await;
                let entries = self.entries.lock().await;
                let input_bytes: u64 = entries.values().map(|e| e.document.byte_size).sum();
                if entries.len() >= 8 || input_bytes > 64 * 1024 * 1024 - INPUT_LIMIT {
                    return Err("document_cache_full_close_earlier_document".into());
                }
                drop(entries);
                let extension = Path::new(path)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if ![
                    "txt", "md", "html", "htm", "csv", "pdf", "docx", "xlsx", "pptx", "doc", "xls",
                    "ppt", "odt", "ods", "odp", "rtf", "epub", "png", "jpg", "jpeg",
                ]
                .contains(&extension.as_str())
                {
                    return Err("unsupported_format".into());
                }
                self.sdk_call(json!({"action":"authorize","path":path}))
                    .await?;
                let (staged, metadata) = self
                    .files
                    .snapshot(path, INPUT_LIMIT)
                    .await
                    .map_err(|e| e.0.code)?;
                if cancelled.load(Ordering::Acquire) {
                    return Err("cancelled".into());
                }
                let directory = self.root.join(uuid::Uuid::new_v4().to_string());
                std::fs::create_dir(&directory).map_err(|_| "document_cache_unavailable")?;
                let snapshot = Arc::new(Snapshot { directory });
                std::fs::rename(&staged.0, snapshot.directory.join("input"))
                    .map_err(|_| "document_snapshot_failed")?;
                let output = self.sdk_call(json!({"action":"extract","directory":snapshot.directory,"extension":extension,"ocr_mode":ocr_mode})).await?;
                if output.get("ready") != Some(&json!(true)) {
                    return Err("document_worker_failed".into());
                }
                let raw = bounded_read(&snapshot.directory.join("result.json"), RESULT_LIMIT)?;
                let mut document: Document =
                    serde_json::from_slice(&raw).map_err(|_| "document_worker_result_invalid")?;
                std::fs::remove_file(snapshot.directory.join("result.json"))
                    .map_err(|_| "document_cache_cleanup_failed")?;
                if metadata["sha256"].as_str() != Some(&document.sha256)
                    || metadata["size"].as_u64() != Some(document.byte_size)
                {
                    return Err("document_snapshot_hash_mismatch".into());
                }
                document.warnings.push("Android: offline OCR uses the bundled simplified-Chinese model with Latin characters; previews support PDF and PNG/JPEG; snapshot lifetime is 15 minutes; execution is the application service UID".into());
                let mut entries = self.entries.lock().await;
                if entries.contains_key(&request.request_id) {
                    return Err("document_reference_conflict".into());
                }
                if entries
                    .values()
                    .map(|e| query::document_cache_size(&e.document))
                    .sum::<usize>()
                    + query::document_cache_size(&document)
                    > CACHE_LIMIT
                {
                    return Err("document_cache_full".into());
                }
                reply.data = Some(query::overview(&document));
                entries.insert(
                    request.request_id,
                    Entry {
                        owner: owner.into(),
                        source: path.clone(),
                        snapshot,
                        document: Arc::new(document),
                        created: Instant::now(),
                        previews: Vec::new(),
                    },
                );
                reply.document_ref = Some(request.request_id);
            }
            DocumentAction::Close { document_ref } => {
                self.sdk_call(json!({"action":"authorize"})).await?;
                if let Some(job) = self.jobs.lock().await.get(&(owner.into(), *document_ref)) {
                    job.store(true, Ordering::Release);
                }
                self.remove(owner, *document_ref).await;
            }
            action => {
                let reference = match action {
                    DocumentAction::Read { document_ref, .. }
                    | DocumentAction::Search { document_ref, .. }
                    | DocumentAction::RenderPage { document_ref, .. } => *document_ref,
                    _ => unreachable!(),
                };
                let (source, snapshot, document) = {
                    let entries = self.entries.lock().await;
                    let entry = entries
                        .get(&reference)
                        .filter(|e| e.owner == owner && e.created.elapsed() < LIFETIME)
                        .ok_or("document_expired_or_not_found")?;
                    (
                        entry.source.clone(),
                        entry.snapshot.clone(),
                        entry.document.clone(),
                    )
                };
                // Revalidate persisted directory permission, even for already cached text.
                self.sdk_call(json!({"action":"authorize","path":source}))
                    .await?;
                match action {
                    DocumentAction::Read {
                        selector, cursor, ..
                    } => {
                        return Ok(query::read_document(
                            reply,
                            reference,
                            &document,
                            selector,
                            cursor.as_deref(),
                        ));
                    }
                    DocumentAction::Search {
                        query: text,
                        case_sensitive,
                        limit,
                        cursor,
                        ..
                    } => {
                        return Ok(query::search_document(
                            reply,
                            reference,
                            &document,
                            text,
                            *case_sensitive,
                            *limit,
                            cursor.as_deref(),
                        ));
                    }
                    DocumentAction::RenderPage {
                        page_number,
                        max_dimension,
                        ..
                    } => {
                        let _worker = self
                            .worker
                            .clone()
                            .try_acquire_owned()
                            .map_err(|_| "document_worker_busy")?;
                        let extension = match document.format.as_str() {
                            "application/pdf" => "pdf",
                            "image/png" => "png",
                            "image/jpeg" => "jpg",
                            _ => {
                                return Err(
                                    "preview_supports_pdf_and_images_only_on_android".into()
                                );
                            }
                        };
                        if *page_number as usize > document.pages.len() {
                            return Err("page_not_found".into());
                        }
                        self.sdk_call(json!({"action":"render","directory":snapshot.directory,"extension":extension,"page_number":page_number,"max_dimension":max_dimension,"sha256":document.sha256})).await?;
                        let png =
                            bounded_read(&snapshot.directory.join("preview.png"), 8 * 1024 * 1024)?;
                        if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
                            return Err("preview_invalid".into());
                        }
                        let hash = crate::filesystem::digest(&png);
                        let id = uuid::Uuid::new_v4();
                        let stage = self.files.stage(id);
                        std::fs::rename(snapshot.directory.join("preview.png"), &stage.0)
                            .map_err(|_| "preview_staging_failed")?;
                        let path = format!("/workspace/pixels-document-preview-{id}.png");
                        sqlx::query("INSERT INTO document_previews(path,hash) VALUES(?,?)")
                            .bind(&path)
                            .bind(&hash)
                            .execute(&self.pool)
                            .await
                            .map_err(|_| "preview_journal_unavailable")?;
                        self.files.io(json!({"action":"publish","path":path,"staging_id":id,"overwrite":false})).await.map_err(|e| e.0.code)?;
                        let mut entries = self.entries.lock().await;
                        if entries.get(&reference).is_none_or(|e| e.owner != owner) {
                            drop(entries);
                            self.delete_preview((path, hash)).await;
                            return Err("document_closed_during_render".into());
                        }
                        let entry = entries.get_mut(&reference).expect("checked document owner");
                        entry.previews.push((path.clone(), hash.clone()));
                        let old = if entry.previews.len() > 4 {
                            Some(entry.previews.remove(0))
                        } else {
                            None
                        };
                        drop(entries);
                        if let Some(old) = old {
                            self.delete_preview(old).await;
                        }
                        reply.document_ref = Some(reference);
                        reply.data = Some(
                            json!({"path":path,"page_number":page_number,"mime_type":"image/png","size":png.len(),"sha256":hash}),
                        );
                    }
                    _ => unreachable!(),
                }
            }
        }
        reply.state = "completed".into();
        Ok(reply)
    }
    async fn sdk_call(&self, command: Value) -> Result<Value, String> {
        let value = self.sdk.call(command).await.map_err(str::to_owned)?;
        if let Some(error) = value["error"].as_str() {
            return Err(error.into());
        }
        Ok(value)
    }
    async fn delete_preview(&self, preview: (String, String)) {
        let result = self
            .files
            .io(json!({"action":"delete","path":preview.0,"expected_hash":preview.1}))
            .await;
        // An externally changed preview belongs to the user's new contents. Keep it.
        if result.is_ok()
            || result.is_err_and(|e| matches!(e.0.code.as_str(), "not_found" | "hash_conflict"))
        {
            let _ = sqlx::query("DELETE FROM document_previews WHERE path=? AND hash=?")
                .bind(preview.0)
                .bind(preview.1)
                .execute(&self.pool)
                .await;
        }
    }
    async fn cleanup_orphan_previews(&self) {
        let Ok(rows) = sqlx::query("SELECT path,hash FROM document_previews")
            .fetch_all(&self.pool)
            .await
        else {
            return;
        };
        if !self.jobs.lock().await.is_empty() {
            return;
        }
        let live: std::collections::HashSet<_> = self
            .entries
            .lock()
            .await
            .values()
            .flat_map(|e| e.previews.iter().map(|p| p.0.clone()))
            .collect();
        for row in rows {
            let path: String = row.get("path");
            if !live.contains(&path) {
                self.delete_preview((path, row.get("hash"))).await;
            }
        }
    }
    async fn remove(&self, owner: &str, reference: RequestId) {
        let mut entries = self.entries.lock().await;
        if entries.get(&reference).is_none_or(|e| e.owner != owner) {
            return;
        }
        let entry = entries.remove(&reference).unwrap();
        drop(entries);
        for preview in entry.previews {
            self.delete_preview(preview).await;
        }
    }
    async fn expire(&self) {
        let old: Vec<_> = self
            .entries
            .lock()
            .await
            .iter()
            .filter(|(_, e)| e.created.elapsed() >= LIFETIME)
            .map(|(id, e)| (e.owner.clone(), *id))
            .collect();
        for (owner, id) in old {
            self.remove(&owner, id).await;
        }
    }
}
fn bounded_read(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| "document_worker_output_missing")?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "document_worker_output_failed")?;
    if bytes.len() as u64 > limit {
        return Err("document_worker_output_limit".into());
    }
    Ok(bytes)
}
fn failed(mut reply: DocumentReply, code: &str) -> DocumentReply {
    reply.state = "failed".into();
    reply.data = None;
    reply.error = Some(code.chars().take(256).collect());
    reply
}

#[cfg(test)]
#[path = "documents_tests.rs"]
mod tests;
