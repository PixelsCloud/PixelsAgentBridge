//! Existing filesystem wire contract backed by the Android SDK storage provider.
//! The journal stores acceptance and outcomes, never text payloads or file contents.
use crate::storage::StorageBridge;
use pab_file_codec::{FileError, decode, encode, newline, read};
use pab_protocol::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::{Mutex, Notify, Semaphore};
use uuid::Uuid;

struct Cached<T> {
    created: tokio::time::Instant,
    value: T,
}
type ResultCache<T> = Mutex<HashMap<(String, RequestId), Cached<T>>>;
impl<T: Clone> Cached<T> {
    fn get(&self) -> Option<T> {
        (self.created.elapsed() < Duration::from_secs(15 * 60)).then(|| self.value.clone())
    }
    fn new(value: T) -> Self {
        Self {
            created: tokio::time::Instant::now(),
            value,
        }
    }
}

pub(crate) struct FileService {
    pub storage: Arc<StorageBridge>,
    pub authorization: Arc<StorageBridge>,
    pool: SqlitePool,
    staging: PathBuf,
    digest_key: [u8; 32],
    acceptance: Mutex<()>,
    execution: Mutex<()>,
    results: ResultCache<Vec<u8>>,
    searches: ResultCache<FileSearchSummary>,
    jobs: Mutex<HashMap<(String, RequestId), Arc<AtomicBool>>>,
    unconfirmed: Mutex<HashSet<(String, RequestId)>>,
    changed: Notify,
    slots: Arc<Semaphore>,
    requests: Arc<Semaphore>,
    active_transfers: Arc<std::sync::Mutex<HashSet<(String, RequestId)>>>,
    transfer_stops: Arc<std::sync::Mutex<HashMap<(String, RequestId), tokio::sync::watch::Sender<FileTransferIntent>>>>,
    native_context: std::sync::OnceLock<ExecutionContext>,
    managed_directories: Mutex<HashMap<RequestId, managed::Directory>>,
}

pub(crate) struct StagedFile(pub PathBuf);
impl Drop for StagedFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl FileService {
    pub async fn open(
        database: &Path,
        staging: &Path,
        digest_key: [u8; 32],
    ) -> Result<Arc<Self>, &'static str> {
        if !staging.is_absolute() || !staging.is_dir() {
            return Err("invalid_staging_directory");
        }
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(database)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Full);
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .map_err(|_| "file_journal_unavailable")?;
        sqlx::query("CREATE TABLE IF NOT EXISTS file_operations(owner TEXT NOT NULL,id TEXT NOT NULL,digest TEXT NOT NULL,reply TEXT NOT NULL,PRIMARY KEY(owner,id)) WITHOUT ROWID")
            .execute(&pool).await.map_err(|_| "file_journal_unavailable")?;
        sqlx::query("UPDATE file_operations SET reply=json_set(reply,'$.state','unconfirmed','$.error',json_object('code','process_interrupted','phase','recover','message','Query the original operation; do not replay an uncertain write')) WHERE json_extract(reply,'$.state') IN ('running','cancel_requested')")
            .execute(&pool).await.map_err(|_| "file_journal_unavailable")?;
        transfer::initialize(&pool).await?;
        Ok(Arc::new(Self {
            storage: StorageBridge::new(),
            authorization: StorageBridge::for_domain("storage_control"),
            pool,
            staging: staging.into(),
            digest_key,
            acceptance: Mutex::new(()),
            execution: Mutex::new(()),
            results: Mutex::new(HashMap::new()),
            searches: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            unconfirmed: Mutex::new(HashSet::new()),
            changed: Notify::new(),
            slots: Arc::new(Semaphore::new(16)),
            requests: Arc::new(Semaphore::new(16)),
            active_transfers: Default::default(),
            transfer_stops: Default::default(),
            native_context: Default::default(),
            managed_directories: Default::default(),
        }))
    }

    pub async fn request(
        self: &Arc<Self>,
        owner: &str,
        request: FileSystemRequest,
        payload: Vec<u8>,
    ) -> (FileSystemReply, Vec<u8>) {
        let Ok(permit) = self.requests.clone().try_acquire_owned() else {
            return failed(
                FileSystemReply::pending(&request),
                "storage_busy",
                "Sixteen request admissions are active",
            );
        };
        let service = self.clone();
        let actor = owner.to_owned();
        let pending = FileSystemReply::pending(&request);
        // Acceptance and worker dispatch survive a dropped network waiter.
        match tokio::spawn(async move {
            let _permit = permit;
            service.request_inner(&actor, request, payload).await
        })
        .await
        {
            Ok(result) => result,
            Err(_) => failed(
                pending,
                "request_outcome_unconfirmed",
                "Query the original request; admission was interrupted",
            ),
        }
    }

    async fn request_inner(
        self: &Arc<Self>,
        owner: &str,
        request: FileSystemRequest,
        payload: Vec<u8>,
    ) -> (FileSystemReply, Vec<u8>) {
        let mut reply = FileSystemReply::pending(&request);
        reply.cancellable = Some(true);
        if request.operation.asynchronous() {
            reply.progress = Some(FileHashProgress {
                completed_bytes: 0,
                total_bytes: 0,
                updated_at_unix_ms: crate::peer::now_ms(),
            });
        }
        if let Err(error) = request.validate() {
            return failed(reply, "invalid_request", error);
        }
        if request.schema_version() > 11 || !request.execution.is_service() {
            return failed(
                reply,
                "unsupported",
                "Android storage executes only as the application UID",
            );
        }
        if payload.len() != request.payload_size as usize
            || (request.operation.has_payload()
                && request.payload_sha256.as_deref() != Some(digest(&payload).as_str()))
        {
            return failed(
                reply,
                "invalid_payload",
                "File payload size or hash differs",
            );
        }
        let key = blake3::keyed_hash(&self.digest_key, &serde_json::to_vec(&request).unwrap())
            .to_hex()
            .to_string();
        let permit;
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let _guard = self.acceptance.lock().await;
            let existing =
                sqlx::query("SELECT digest,reply FROM file_operations WHERE owner=? AND id=?")
                    .bind(owner)
                    .bind(request.request_id.to_string())
                    .fetch_optional(&self.pool)
                    .await;
            match existing {
                Ok(Some(row)) => {
                    if row.get::<String, _>("digest") != key {
                        return failed(
                            reply,
                            "request_conflict",
                            "Original request parameters differ",
                        );
                    }
                    let stored: FileSystemReply = match serde_json::from_str(row.get("reply")) {
                        Ok(value) => value,
                        Err(_) => {
                            return failed(
                                reply,
                                "journal_invalid",
                                "Original outcome is unavailable",
                            );
                        }
                    };
                    if stored.state == "running" {
                        return (stored, Vec::new());
                    }
                    drop(_guard);
                    return self.result(owner, request.request_id).await;
                }
                Err(_) => {
                    return failed(
                        reply,
                        "journal_unavailable",
                        "File acceptance could not be read",
                    );
                }
                Ok(None) => {}
            }
            let transfer_count: i64 =
                sqlx::query_scalar("SELECT count(*) FROM file_transfers WHERE owner=? AND id=?")
                    .bind(owner)
                    .bind(request.request_id.to_string())
                    .fetch_one(&self.pool)
                    .await
                    .unwrap_or(1);
            if transfer_count > 0 {
                return failed(
                    reply,
                    "request_conflict",
                    "ID already belongs to a transfer",
                );
            }
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM file_operations")
                .fetch_one(&self.pool)
                .await
                .unwrap_or(10000);
            if count >= 10000 {
                return failed(
                    reply,
                    "journal_capacity",
                    "File request journal is full; existing outcomes are preserved",
                );
            }
            permit = match Arc::clone(&self.slots).try_acquire_owned() {
                Ok(value) => value,
                Err(_) => {
                    return failed(
                        reply,
                        "storage_busy",
                        "Sixteen file operations are already accepted",
                    );
                }
            };
            if sqlx::query("INSERT INTO file_operations(owner,id,digest,reply) VALUES(?,?,?,?)")
                .bind(owner)
                .bind(request.request_id.to_string())
                .bind(key)
                .bind(serde_json::to_string(&reply).unwrap())
                .execute(&self.pool)
                .await
                .is_err()
            {
                return failed(
                    reply,
                    "journal_unavailable",
                    "File request was not accepted",
                );
            }
            self.jobs
                .lock()
                .await
                .insert((owner.into(), request.request_id), cancel.clone());
        }
        let worker = Arc::clone(self);
        let actor = owner.to_owned();
        let id = request.request_id;
        let asynchronous = request.operation.asynchronous();
        let accepted = reply.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _guard = worker.execution.lock().await;
            let result = match tokio::time::timeout(
                Duration::from_secs(30 * 60),
                worker.perform(&actor, &request, &payload, &mut reply, &cancel),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => {
                    cancel.store(true, Ordering::Release);
                    if let Some(summary) = &mut reply.mutation {
                        summary.partial =
                            summary.published_entries > 0 || summary.deleted_entries > 0;
                    }
                    Err(FileError::new(
                        if request.operation.mutates() {
                            "time_limit_unconfirmed"
                        } else {
                            "time_limit"
                        },
                        "execute",
                        "Thirty-minute worker deadline reached; query the original operation",
                    ))
                }
            };
            match result {
                Ok(data) => {
                    if cancel.load(Ordering::Acquire) && !request.operation.mutates() {
                        reply.state = "cancelled".into();
                        reply.error = Some(
                            FileError::new(
                                "cancelled",
                                "execute",
                                "Read result discarded after cancellation",
                            )
                            .0,
                        );
                    } else {
                        reply.state = "completed".into();
                        if !data.is_empty() {
                            reply.data_size = data.len() as u32;
                            reply.data_sha256 = Some(digest(&data));
                        }
                        if let Some(search) = &reply.search {
                            let mut searches = worker.searches.lock().await;
                            searches.retain(|_, value| {
                                value.created.elapsed() < Duration::from_secs(900)
                            });
                            if searches.len() >= 64
                                && let Some(key) = searches
                                    .iter()
                                    .min_by_key(|(_, v)| v.created)
                                    .map(|(k, _)| k.clone())
                            {
                                searches.remove(&key);
                            }
                            searches.insert((actor.clone(), id), Cached::new(search.clone()));
                        }
                        let mut cache = worker.results.lock().await;
                        if request.operation.kind() == "file_read" {
                            cache.retain(|_, value| {
                                value.created.elapsed() < Duration::from_secs(900)
                            });
                            if cache.len() >= 64
                                && let Some(key) = cache
                                    .iter()
                                    .min_by_key(|(_, v)| v.created)
                                    .map(|(k, _)| k.clone())
                            {
                                cache.remove(&key);
                            }
                            cache.insert((actor.clone(), id), Cached::new(data));
                        }
                    }
                }
                Err(error) => {
                    reply.state = if error.0.code == "cancelled" {
                        "cancelled"
                    } else if error.0.code.contains("unconfirmed") {
                        "unconfirmed"
                    } else {
                        "failed"
                    }
                    .into();
                    reply.error = Some(error.0);
                }
            }
            // Search previews are transient content, like text read payloads.
            if let Some(search) = &mut reply.search {
                for item in &mut search.matches {
                    item.preview = None;
                    item.context.clear();
                }
            }
            if sqlx::query("UPDATE file_operations SET reply=? WHERE owner=? AND id=?")
                .bind(serde_json::to_string(&reply).unwrap())
                .bind(&actor)
                .bind(id.to_string())
                .execute(&worker.pool)
                .await
                .is_err()
            {
                worker.unconfirmed.lock().await.insert((actor.clone(), id));
            }
            worker.jobs.lock().await.remove(&(actor, id));
            worker.changed.notify_waiters();
        });
        if asynchronous {
            return (accepted, Vec::new());
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let notified = self.changed.notified();
            let stored = self.lookup(owner, id).await;
            if stored.state != "running" || tokio::time::Instant::now() >= deadline {
                return if stored.state == "completed" {
                    self.result(owner, id).await
                } else {
                    (stored, Vec::new())
                };
            }
            let _ = tokio::time::timeout_at(deadline, notified).await;
        }
    }

    pub async fn lookup(&self, owner: &str, id: RequestId) -> FileSystemReply {
        let record = sqlx::query_scalar::<_, String>(
            "SELECT reply FROM file_operations WHERE owner=? AND id=?",
        )
        .bind(owner)
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await;
        if let Ok(Some(value)) = &record
            && let Ok(mut reply) = serde_json::from_str::<FileSystemReply>(value)
        {
            if self.unconfirmed.lock().await.contains(&(owner.into(), id)) {
                reply.state = "unconfirmed".into();
                reply.error = Some(
                    FileError::new(
                        "journal_unavailable",
                        "record",
                        "Worker stopped but its final outcome could not be recorded; do not replay",
                    )
                    .0,
                );
                return reply;
            }
            if let Some(body)=&reply.file_manager {
                if let Err(error)=self.authorize_managed_reply(body).await {
                    reply.file_manager=None;reply.state="failed".into();reply.error=Some(error.0);return reply;
                }
            }
            if let Some(search) = self
                .searches
                .lock()
                .await
                .get(&(owner.into(), id))
                .and_then(Cached::get)
            {
                reply.search = Some(search);
            }
            return reply;
        }
        let request = FileSystemRequest {
            execution: ExecutionSelection::default(),
            request_id: id,
            path: String::new(),
            operation: FileSystemAction::Stat {
                follow_symlinks: false,
            },
            payload_size: 0,
            payload_sha256: None,
        };
        let (code, message) = match record {
            Ok(None) => (
                "not_found",
                "Original file operation was not found for this caller",
            ),
            Ok(Some(_)) => (
                "journal_invalid",
                "Original outcome cannot be decoded; do not replay",
            ),
            Err(_) => (
                "journal_unavailable",
                "Original outcome cannot be read; do not replay",
            ),
        };
        let mut reply = failed(FileSystemReply::pending(&request), code, message).0;
        if code != "not_found" {
            reply.state = "unconfirmed".into();
        }
        reply
    }

    pub async fn result(&self, owner: &str, id: RequestId) -> (FileSystemReply, Vec<u8>) {
        let mut reply = self.lookup(owner, id).await;
        if reply.state != "completed" {
            reply.data_size = 0;
            reply.data_sha256 = None;
            return (reply, Vec::new());
        }
        let content = reply.kind == "file_read" || reply.kind == "file_search";
        if content {
            let authorization = tokio::time::timeout(
                Duration::from_secs(2),
                self.authorization
                    .call(json!({"action":"authorize","path":reply.path})),
            )
            .await;
            let error = match authorization {
                Ok(Ok(value)) => value["error"].as_str().map(str::to_owned),
                _ => Some("storage_authorization_unavailable".into()),
            };
            if let Some(error) = error {
                reply.search = None;
                return result_failed(
                    reply,
                    &error,
                    "Current control or mount authorization denied",
                );
            }
        }
        if reply.kind == "file_read" {
            let data = self
                .results
                .lock()
                .await
                .get(&(owner.into(), id))
                .and_then(Cached::get);
            let Some(data) = data else {
                return result_failed(
                    reply,
                    "result_expired",
                    "Original content expired; no read was replayed",
                );
            };
            if data.len() != reply.data_size as usize
                || (reply.data_sha256.is_some()
                    && reply.data_sha256.as_deref() != Some(digest(&data).as_str()))
            {
                return result_failed(
                    reply,
                    "result_unavailable",
                    "Original content integrity is unavailable",
                );
            }
            reply.data_sha256 = Some(digest(&data));
            return (reply, data);
        }
        if reply.kind == "file_search"
            && self
                .searches
                .lock()
                .await
                .get(&(owner.into(), id))
                .and_then(Cached::get)
                .is_none()
        {
            reply.search = None;
            return result_failed(
                reply,
                "result_expired",
                "Original search content expired; no search was replayed",
            );
        }
        reply.data_size = 0;
        reply.data_sha256 = None;
        (reply, Vec::new())
    }

    pub async fn cancel(&self, owner: &str, id: RequestId) -> FileSystemReply {
        let _guard = self.acceptance.lock().await;
        let mut reply = self.lookup(owner, id).await;
        if !reply.supports_cancel()
            || !matches!(reply.state.as_str(), "running" | "cancel_requested")
        {
            return reply;
        }
        let jobs = self.jobs.lock().await;
        let Some(cancel) = jobs.get(&(owner.into(), id)) else {
            return reply;
        };
        cancel.store(true, Ordering::Release);
        if sqlx::query("UPDATE file_operations SET reply=json_set(reply,'$.state','cancel_requested') WHERE owner=? AND id=? AND json_extract(reply,'$.state') IN ('running','cancel_requested')")
            .bind(owner).bind(id.to_string()).execute(&self.pool).await.is_err() {
            reply.state = "unconfirmed".into();
            reply.error = Some(FileError::new("journal_unavailable", "cancel", "Cancellation was requested but its acknowledgement could not be stored").0);
            return reply;
        }
        self.lookup(owner, id).await
    }

    pub(crate) async fn io(&self, body: Value) -> Result<Value, FileError> {
        let value = self
            .storage
            .call(body)
            .await
            .map_err(|error| FileError::new(error, "storage", error))?;
        if let Some(code) = value.get("error").and_then(Value::as_str) {
            return Err(FileError::new(code, "storage", code));
        }
        Ok(value)
    }

    fn seal_cursor(
        &self,
        owner: &str,
        path: &str,
        purpose: &str,
        payload: &str,
    ) -> Result<String, FileError> {
        let issued = crate::peer::now_ms();
        let bytes = serde_json::to_vec(&(owner, path, purpose, issued, payload))
            .map_err(|_| FileError::new("invalid_cursor", "encode", "Cursor encoding failed"))?;
        Ok(json!({"version":1,"issued":issued,"payload":payload,"proof":blake3::keyed_hash(&self.digest_key,&bytes).to_hex().to_string()}).to_string())
    }
    fn open_cursor(
        &self,
        owner: &str,
        path: &str,
        purpose: &str,
        token: &str,
    ) -> Result<String, FileError> {
        let invalid = || {
            FileError::new(
                "invalid_cursor",
                "validate",
                "Cursor expired or does not belong to this caller, path and query",
            )
        };
        let value: Value = serde_json::from_str(token).map_err(|_| invalid())?;
        let issued = value["issued"].as_i64().ok_or_else(invalid)?;
        let elapsed = crate::peer::now_ms().saturating_sub(issued);
        let payload = value["payload"].as_str().ok_or_else(invalid)?;
        if value["version"] != 1 || !(0..=900000).contains(&elapsed) {
            return Err(invalid());
        }
        let bytes =
            serde_json::to_vec(&(owner, path, purpose, issued, payload)).map_err(|_| invalid())?;
        let proof = blake3::Hash::from_hex(value["proof"].as_str().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        if proof != blake3::keyed_hash(&self.digest_key, &bytes) {
            return Err(invalid());
        }
        Ok(payload.to_owned())
    }

    pub fn stage(&self, id: Uuid) -> StagedFile {
        StagedFile(self.staging.join(format!("{id}.part")))
    }

    pub async fn snapshot(&self, path: &str, limit: u64) -> Result<(StagedFile, Value), FileError> {
        let id = Uuid::new_v4();
        let stage = self.stage(id);
        let value = self
            .io(json!({"action":"snapshot","path":path,"staging_id":id,"maximum_bytes":limit}))
            .await?;
        Ok((stage, value))
    }

    async fn perform(
        &self,
        owner: &str,
        request: &FileSystemRequest,
        payload: &[u8],
        reply: &mut FileSystemReply,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Vec<u8>, FileError> {
        cancelled(cancel)?;
        match &request.operation {
            FileSystemAction::Managed { operation, expected_execution } => {
                self.managed(owner,request,operation,expected_execution.as_ref(),reply,cancel).await?;
                Ok(Vec::new())
            }
            operation if operation.is_bulk() => self.bulk(owner, request, reply, cancel).await,
            FileSystemAction::Search { .. } => {
                reply.search = Some(self.search(owner, request, cancel).await?);
                Ok(Vec::new())
            }
            FileSystemAction::Hash => self.hash(owner, request, reply, cancel).await,
            FileSystemAction::Mkdir { parents, exist_ok } => {
                self.mkdir(&request.path, *parents, *exist_ok, reply)
                    .await?;
                Ok(Vec::new())
            }
            FileSystemAction::Stat { .. } => {
                let value = self
                    .io(json!({"action":"stat","path":request.path}))
                    .await?;
                reply.metadata = Some(metadata(&value["metadata"]));
                Ok(Vec::new())
            }
            FileSystemAction::Read {
                range,
                encoding,
                expected_hash,
            } => {
                if matches!(range, TextReadRange::Stream { .. }) {
                    return self.log_read(owner, request, reply, cancel).await;
                }
                let (stage, value) = self
                    .snapshot(&request.path, MAX_TEXT_FILE_BYTES as u64)
                    .await?;
                let bytes = tokio::fs::read(&stage.0).await.map_err(io_error)?;
                let hash = digest(&bytes);
                check_hash(expected_hash.as_deref(), &hash)?;
                let doc = decode(&bytes, *encoding)?;
                let (data, position) = read(&doc, range)?;
                let mut meta = metadata(&value["metadata"]);
                meta.sha256 = Some(hash);
                meta.encoding = Some(doc.encoding);
                meta.newline = Some(newline(&doc.text));
                reply.metadata = Some(meta);
                reply.range = Some(position);
                Ok(data)
            }
            FileSystemAction::Write {
                encoding,
                overwrite,
                expected_hash,
            } => {
                let text = std::str::from_utf8(payload).map_err(|_| {
                    FileError::new("invalid_encoding", "write", "Payload must be UTF-8")
                })?;
                let bytes = encode(text, *encoding, true)?;
                let mut meta = self
                    .publish(&request.path, &bytes, *overwrite, expected_hash.as_deref())
                    .await?;
                meta.encoding = Some(*encoding);
                meta.newline = Some(newline(text));
                reply.metadata = Some(meta);
                Ok(Vec::new())
            }
            FileSystemAction::Patch {
                expected_hash,
                encoding,
                dry_run,
            } => {
                let (stage, _) = self
                    .snapshot(&request.path, MAX_TEXT_FILE_BYTES as u64)
                    .await?;
                let bytes = tokio::fs::read(&stage.0).await.map_err(io_error)?;
                check_hash(Some(expected_hash), &digest(&bytes))?;
                let edits: Vec<TextEdit> = serde_json::from_slice(payload).map_err(|_| {
                    FileError::new("invalid_patch", "patch", "Invalid patch payload")
                })?;
                let plan = pab_file_codec::plan_patch(&bytes, *encoding, &edits)?;
                if *dry_run {
                    reply.patch_preview = Some(plan.preview);
                    return Ok(Vec::new());
                }
                let mut meta = self
                    .publish(&request.path, &plan.bytes, true, Some(expected_hash))
                    .await?;
                meta.encoding = Some(plan.encoding);
                meta.newline = Some(plan.newline);
                reply.metadata = Some(meta);
                Ok(Vec::new())
            }
            _ => Err(FileError::new(
                "unsupported",
                "validate",
                "File action is not yet implemented",
            )),
        }
    }

    async fn mkdir(
        &self,
        path: &str,
        parents: bool,
        exist_ok: bool,
        reply: &mut FileSystemReply,
    ) -> Result<(), FileError> {
        let path = bulk::normalized(path)?;
        let parts = path[1..].split('/').collect::<Vec<_>>();
        if parts.len() > 129 {
            return Err(bulk::limit("depth_limit"));
        }
        reply.created_paths = Some(Vec::new());
        let mut current = format!("/{}", parts[0]);
        if parents {
            for part in &parts[1..] {
                current.push('/');
                current.push_str(part);
                match self.io(json!({"action":"stat","path":current})).await {
                    Ok(value) => {
                        if value["metadata"]["directory"] != true {
                            return Err(bulk::limit("not_a_directory"));
                        }
                        if current == path && !exist_ok {
                            return Err(bulk::limit("already_exists"));
                        }
                    }
                    Err(error) if error.0.code == "not_found" => {
                        self.io(json!({"action":"mkdir","path":current,"parents":false,"exist_ok":false})).await?;
                        reply.created_paths.as_mut().unwrap().push(current.clone());
                    }
                    Err(error) => return Err(error),
                }
            }
            if parts.len() == 1 && !exist_ok {
                return Err(bulk::limit("already_exists"));
            }
        } else {
            let before = self.io(json!({"action":"stat","path":path})).await;
            let missing = match before {
                Ok(_) => false,
                Err(error) if error.0.code == "not_found" => true,
                Err(error) => return Err(error),
            };
            let value = self
                .io(json!({"action":"mkdir","path":path,"parents":false,"exist_ok":exist_ok}))
                .await?;
            if missing {
                reply.created_paths.as_mut().unwrap().push(path.clone());
            }
            reply.metadata = Some(metadata(&value["metadata"]));
            return Ok(());
        }
        let value = self.io(json!({"action":"stat","path":path})).await?;
        reply.metadata = Some(metadata(&value["metadata"]));
        Ok(())
    }

    async fn publish(
        &self,
        path: &str,
        bytes: &[u8],
        overwrite: bool,
        expected: Option<&str>,
    ) -> Result<FileMetadata, FileError> {
        let id = Uuid::new_v4();
        let stage = self.stage(id);
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&stage.0)
            .await
            .map_err(io_error)?;
        use tokio::io::AsyncWriteExt;
        file.write_all(bytes).await.map_err(io_error)?;
        file.sync_all().await.map_err(io_error)?;
        drop(file);
        let value = self.io(json!({"action":"publish","path":path,"staging_id":id,"overwrite":overwrite,"expected_hash":expected})).await?;
        let mut meta = metadata(&value["metadata"]);
        meta.sha256 = Some(digest(bytes));
        Ok(meta)
    }

    pub async fn directory(
        &self,
        id: RequestId,
        path: &str,
        after: Option<String>,
        limit: u16,
    ) -> Result<DirectoryPage, FileError> {
        if limit == 0
            || limit > MAX_DIRECTORY_PAGE_ENTRIES
            || path.len() > 4096
            || after.as_ref().is_some_and(|a| a.len() > 255)
        {
            return Err(FileError::new(
                "invalid_request",
                "list",
                "Invalid directory page request",
            ));
        }
        let value = self
            .io(json!({"action":"list","path":path,"after":after,"limit":limit}))
            .await?;
        let mut entries = Vec::new();
        for item in value["entries"]
            .as_array()
            .ok_or_else(|| FileError::new("invalid_result", "list", "Missing entries"))?
        {
            let name = item["name"]
                .as_str()
                .ok_or_else(|| FileError::new("invalid_result", "list", "Missing entry name"))?;
            if name.is_empty() || name.len() > 255 || name.contains(['/', '\\', '\0']) {
                return Err(FileError::new(
                    "invalid_result",
                    "list",
                    "Invalid entry name",
                ));
            }
            entries.push(DirectoryEntry {
                name: name.into(),
                kind: if item["directory"] == true {
                    DirectoryEntryKind::Directory
                } else {
                    DirectoryEntryKind::File
                },
                size: item["size"].as_u64(),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let next_after = if value["has_more"] == true {
            entries.last().map(|e| e.name.clone())
        } else {
            None
        };
        Ok(DirectoryPage {
            execution_context: None,
            request_id: id,
            path: path.into(),
            entries,
            next_after,
        })
    }
}

fn cancelled(cancel: &AtomicBool) -> Result<(), FileError> {
    if cancel.load(Ordering::Acquire) {
        Err(FileError::new(
            "cancelled",
            "execute",
            "Operation stopped after cancellation",
        ))
    } else {
        Ok(())
    }
}

fn metadata(value: &Value) -> FileMetadata {
    FileMetadata {
        kind: if value["directory"] == true {
            "directory"
        } else {
            "file"
        }
        .into(),
        size: value["size"].as_u64().unwrap_or(0),
        modified_at_unix_ms: value["modified_at_unix_ms"].as_i64(),
        readonly: value["readonly"].as_bool().unwrap_or(true),
        unix_mode: None,
        is_link: false,
        link_target: None,
        sha256: None,
        encoding: None,
        newline: None,
    }
}
fn io_error(error: std::io::Error) -> FileError {
    FileError::new("storage_io_error", "staging", error.to_string())
}
fn check_hash(expected: Option<&str>, actual: &str) -> Result<(), FileError> {
    if expected.is_some_and(|e| e != actual) {
        Err(FileError::new(
            "hash_conflict",
            "validate",
            "File has changed",
        ))
    } else {
        Ok(())
    }
}
fn failed(mut reply: FileSystemReply, code: &str, message: &str) -> (FileSystemReply, Vec<u8>) {
    reply.state = "failed".into();
    reply.error = Some(FileError::new(code, "accept", message).0);
    reply.data_size = 0;
    reply.data_sha256 = None;
    (reply, Vec::new())
}

#[cfg(test)]
#[path = "filesystem_enhanced_tests.rs"]
mod enhanced_tests;
fn result_failed(reply: FileSystemReply, code: &str, message: &str) -> (FileSystemReply, Vec<u8>) {
    let (mut reply, data) = failed(reply, code, message);
    if let Some(error) = reply.error.as_mut() {
        error.phase = "result".into();
    }
    (reply, data)
}

#[cfg(test)]
#[path = "filesystem_tests.rs"]
mod tests;

#[path = "file_transfer.rs"]
mod transfer;

#[path = "file_wire.rs"]
mod wire;

#[path = "file_search.rs"]
mod search;

#[path = "file_log.rs"]
mod log;

#[path = "file_hash.rs"]
mod hash;

#[path = "file_bulk.rs"]
mod bulk;

#[path = "file_archive.rs"]
mod archive;

#[path = "file_managed.rs"]
mod managed;
