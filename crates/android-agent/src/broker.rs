//! Bounded platform mailbox with durable acceptance and no persisted screen/text payloads.
use pab_protocol::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

#[derive(Default)]
pub struct ReferenceScope {
    closed: Arc<AtomicBool>,
    elements: HashMap<String, String>,
    observations: HashMap<String, u64>,
}

pub struct ScopeLease(Arc<AtomicBool>);
impl Drop for ScopeLease {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
impl ReferenceScope {
    pub fn lease(&self) -> ScopeLease {
        ScopeLease(Arc::clone(&self.closed))
    }
}

#[cfg(test)]
#[path = "broker_tests.rs"]
mod tests;

struct Pending {
    owner: String,
    id: RequestId,
    payload: Value,
    scope: Arc<Mutex<ReferenceScope>>,
    mutation: bool,
}

pub struct PlatformBroker {
    pool: SqlitePool,
    digest_key: [u8; 32],
    accepted: Mutex<()>,
    sender: mpsc::Sender<Uuid>,
    receiver: Mutex<mpsc::Receiver<Uuid>>,
    pending: Mutex<HashMap<Uuid, Pending>>,
    results: Mutex<HashMap<(String, RequestId), MobileReply>>,
    image: Mutex<Option<(String, RequestId, Vec<u8>)>>,
}

impl PlatformBroker {
    pub async fn open(path: &Path, digest_key: [u8; 32]) -> Result<Arc<Self>, &'static str> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .page_size(4096)
            .pragma("max_page_count", "65536")
            .pragma("wal_autocheckpoint", "256")
            .pragma("journal_size_limit", "4194304")
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Full);
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .map_err(|_| "storage_unavailable")?;
        let schema: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&pool)
            .await
            .map_err(|_| "storage_unavailable")?;
        let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
            .fetch_one(&pool)
            .await
            .map_err(|_| "storage_unavailable")?;
        let pages: i64 = sqlx::query_scalar("PRAGMA page_count")
            .fetch_one(&pool)
            .await
            .map_err(|_| "storage_unavailable")?;
        if schema > 1 || page_size != 4096 || pages > 65536 {
            pool.close().await;
            return Err("unsupported_operation_store");
        }
        let mut migration = pool.begin().await.map_err(|_| "storage_unavailable")?;
        sqlx::query("CREATE TABLE IF NOT EXISTS mobile_operations (owner TEXT NOT NULL,id TEXT NOT NULL,digest TEXT NOT NULL,reply TEXT NOT NULL,claimed INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(owner,id))").execute(&mut *migration).await.map_err(|_| "storage_unavailable")?;
        sqlx::query("CREATE TABLE IF NOT EXISTS mobile_operation_archive (owner TEXT NOT NULL,id TEXT NOT NULL,digest TEXT NOT NULL,state TEXT NOT NULL,dispatched INTEGER NOT NULL,acknowledged INTEGER NOT NULL,error TEXT,PRIMARY KEY(owner,id)) WITHOUT ROWID").execute(&mut *migration).await.map_err(|_| "storage_unavailable")?;
        sqlx::query("PRAGMA user_version=1")
            .execute(&mut *migration)
            .await
            .map_err(|_| "storage_unavailable")?;
        migration
            .commit()
            .await
            .map_err(|_| "storage_unavailable")?;
        let rows = sqlx::query("SELECT owner,id,reply FROM mobile_operations WHERE claimed=1")
            .fetch_all(&pool)
            .await
            .map_err(|_| "storage_unavailable")?;
        for row in rows {
            let mut reply: MobileReply =
                serde_json::from_str(row.get("reply")).map_err(|_| "invalid_operation_store")?;
            if reply.state == MobileState::Running {
                reply.state = MobileState::Unconfirmed;
                reply.error = Some("process_interrupted".into());
                sqlx::query("UPDATE mobile_operations SET reply=? WHERE owner=? AND id=?")
                    .bind(serde_json::to_string(&reply).unwrap())
                    .bind(row.get::<String, _>("owner"))
                    .bind(row.get::<String, _>("id"))
                    .execute(&pool)
                    .await
                    .map_err(|_| "storage_unavailable")?;
            }
        }
        // Accepted but never claimed work also requires a fresh request after process death.
        sqlx::query("UPDATE mobile_operations SET reply=json_set(reply,'$.state','cancelled','$.error','process_interrupted') WHERE claimed=0 AND json_extract(reply,'$.state')='running'").execute(&pool).await.map_err(|_| "storage_unavailable")?;
        let (sender, receiver) = mpsc::channel(16);
        Ok(Arc::new(Self {
            pool,
            digest_key,
            accepted: Mutex::new(()),
            sender,
            receiver: Mutex::new(receiver),
            pending: Mutex::new(HashMap::new()),
            results: Mutex::new(HashMap::new()),
            image: Mutex::new(None),
        }))
    }

    pub async fn request(
        self: &Arc<Self>,
        owner: &str,
        scope: Arc<Mutex<ReferenceScope>>,
        request: MobileRequest,
    ) -> MobileReply {
        let id = request.request_id();
        match self.request_inner(owner, scope, request).await {
            Ok(reply) => reply,
            Err(code) => MobileReply::failed(id, code),
        }
    }

    async fn request_inner(
        self: &Arc<Self>,
        owner: &str,
        scope: Arc<Mutex<ReferenceScope>>,
        request: MobileRequest,
    ) -> Result<MobileReply, &'static str> {
        let _guard = self.accepted.lock().await;
        let id = request.request_id();
        let mut existing = sqlx::query(
            "SELECT digest,reply,claimed FROM mobile_operations WHERE owner=? AND id=?",
        )
        .bind(owner)
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| "storage_unavailable")?;
        if existing.is_none() {
            existing = self.archived(owner, id).await?;
        }
        match request {
            MobileRequest::Get { .. } => self.lookup(owner, id, existing).await,
            MobileRequest::Cancel { .. } => {
                let row = existing.ok_or("not_found")?;
                let mut reply: MobileReply =
                    serde_json::from_str(row.get("reply")).map_err(|_| "storage_unavailable")?;
                if reply.state == MobileState::Running {
                    if row.get::<i64, _>("claimed") == 0 {
                        reply.state = MobileState::Cancelled;
                    } else {
                        reply.error = Some("cancel_requested_after_dispatch".into());
                    }
                    self.save(owner, &reply).await?;
                }
                Ok(reply)
            }
            MobileRequest::Execute { command, .. } => {
                command.validate()?;
                let digest =
                    blake3::keyed_hash(&self.digest_key, &serde_json::to_vec(&command).unwrap())
                        .to_hex()
                        .to_string();
                if let Some(row) = existing {
                    if row.get::<String, _>("digest") != digest {
                        return Err("request_conflict");
                    }
                    return self.lookup(owner, id, Some(row)).await;
                }
                let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mobile_operations")
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|_| "storage_unavailable")?;
                if count >= 10000 && self.archive_completed().await? == 0 {
                    return Err("operation_store_full");
                }
                let payload = self.translate(&command, &scope).await?;
                let ticket = Uuid::new_v4();
                let permit = self.sender.try_reserve().map_err(|_| "busy")?;
                let reply = MobileReply::pending(id);
                sqlx::query("INSERT INTO mobile_operations(owner,id,digest,reply) VALUES(?,?,?,?)")
                    .bind(owner)
                    .bind(id.to_string())
                    .bind(digest)
                    .bind(serde_json::to_string(&reply).unwrap())
                    .execute(&self.pool)
                    .await
                    .map_err(|_| "storage_unavailable")?;
                self.pending.lock().await.insert(
                    ticket,
                    Pending {
                        owner: owner.into(),
                        id,
                        payload,
                        scope,
                        mutation: command.is_mutation(),
                    },
                );
                permit.send(ticket);
                let broker = Arc::clone(self);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    let _guard = broker.accepted.lock().await;
                    let pending = broker.pending.lock().await.remove(&ticket);
                    if let Some(pending) = pending
                        && let Ok(mut reply) = broker.get(&pending.owner, pending.id).await
                        && reply.state == MobileState::Running
                    {
                        reply.state = if reply.action_dispatched {
                            MobileState::Unconfirmed
                        } else {
                            MobileState::Failed
                        };
                        reply.error = Some("platform_timeout".into());
                        let _ = broker.save(&pending.owner, &reply).await;
                    }
                });
                Ok(reply)
            }
        }
    }

    async fn translate(
        &self,
        command: &MobileCommand,
        scope: &Mutex<ReferenceScope>,
    ) -> Result<Value, &'static str> {
        let refs = scope.lock().await;
        Ok(match command {
            MobileCommand::Observe {} => json!({"action":"observe"}),
            MobileCommand::Screenshot {} => json!({"action":"screenshot"}),
            MobileCommand::Capabilities {} => json!({"action":"capabilities"}),
            MobileCommand::Query {
                root_ref,
                text_contains,
                role,
                max_nodes,
                max_depth,
            } => {
                let root = root_ref
                    .as_ref()
                    .map(|r| refs.elements.get(r).cloned().ok_or("target_stale"))
                    .transpose()?;
                json!({"action":"query","root_reference":root,"text_contains":text_contains,"role":role,"max_nodes":max_nodes,"max_depth":max_depth})
            }
            MobileCommand::LongClick { element_ref } => {
                json!({"action":"long_click","reference":refs.elements.get(element_ref).ok_or("target_stale")?})
            }
            MobileCommand::Scroll {
                element_ref,
                direction,
            } => {
                json!({"action":"scroll","reference":refs.elements.get(element_ref).ok_or("target_stale")?,"direction":direction})
            }
            MobileCommand::LaunchApp { package_name } => {
                json!({"action":"launch_app","package_name":package_name})
            }
            MobileCommand::WaitFor {
                text,
                present,
                timeout_ms,
                interval_ms,
            } => {
                json!({"action":"wait_for","text":text,"present":present,"timeout_ms":timeout_ms,"interval_ms":interval_ms})
            }
            MobileCommand::Click { element_ref } => {
                json!({"action":"click","reference":refs.elements.get(element_ref).ok_or("target_stale")?})
            }
            MobileCommand::SetText { element_ref, text } => {
                json!({"action":"set_text","reference":refs.elements.get(element_ref).ok_or("target_stale")?,"text":text})
            }
            MobileCommand::Gesture {
                observation_ref,
                from_x,
                from_y,
                to_x,
                to_y,
                duration_ms,
            } => {
                json!({"action":"gesture","revision":refs.observations.get(observation_ref).ok_or("target_stale")?,"from_x":from_x,"from_y":from_y,"to_x":to_x,"to_y":to_y,"duration_ms":duration_ms})
            }
            MobileCommand::Navigate { destination } => {
                json!({"action":"navigate","destination":destination})
            }
        })
    }

    async fn lookup(
        &self,
        owner: &str,
        id: RequestId,
        row: Option<sqlx::sqlite::SqliteRow>,
    ) -> Result<MobileReply, &'static str> {
        let row = row.ok_or("not_found")?;
        if let Some(reply) = self.results.lock().await.get(&(owner.into(), id)).cloned() {
            return Ok(reply);
        }
        serde_json::from_str(row.get("reply")).map_err(|_| "invalid_operation_store")
    }

    async fn archived(
        &self,
        owner: &str,
        id: RequestId,
    ) -> Result<Option<sqlx::sqlite::SqliteRow>, &'static str> {
        sqlx::query("SELECT digest,1 AS claimed,json_object('request_id',id,'state',state,'action_dispatched',json(CASE dispatched WHEN 0 THEN 'false' ELSE 'true' END),'platform_acknowledged',json(CASE acknowledged WHEN 0 THEN 'false' ELSE 'true' END),'error',error,'observation',NULL,'image',NULL) AS reply FROM mobile_operation_archive WHERE owner=? AND id=?")
            .bind(owner).bind(id.to_string()).fetch_optional(&self.pool).await.map_err(|_| "storage_unavailable")
    }

    /// Preserve every accepted ID and its digest while compacting terminal metadata.
    /// Copy and removal commit together; full disk rolls back without permitting a replay.
    async fn archive_completed(&self) -> Result<u64, &'static str> {
        let mut tx = self.pool.begin().await.map_err(|_| "storage_unavailable")?;
        let copied = sqlx::query("INSERT INTO mobile_operation_archive(owner,id,digest,state,dispatched,acknowledged,error) SELECT owner,id,digest,json_extract(reply,'$.state'),json_extract(reply,'$.action_dispatched'),json_extract(reply,'$.platform_acknowledged'),json_extract(reply,'$.error') FROM mobile_operations WHERE json_extract(reply,'$.state')!='running' ORDER BY rowid LIMIT 1000")
            .execute(&mut *tx).await.map_err(|_| "storage_maintenance_failed")?.rows_affected();
        sqlx::query("DELETE FROM mobile_operations WHERE rowid IN (SELECT o.rowid FROM mobile_operations o JOIN mobile_operation_archive a ON a.owner=o.owner AND a.id=o.id)")
            .execute(&mut *tx).await.map_err(|_| "storage_maintenance_failed")?;
        tx.commit()
            .await
            .map_err(|_| "storage_maintenance_failed")?;
        Ok(copied)
    }
    async fn get(&self, owner: &str, id: RequestId) -> Result<MobileReply, &'static str> {
        let mut row = sqlx::query("SELECT reply FROM mobile_operations WHERE owner=? AND id=?")
            .bind(owner)
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| "storage_unavailable")?;
        if row.is_none() {
            row = self.archived(owner, id).await?;
        }
        self.lookup(owner, id, row).await
    }
    async fn save(&self, owner: &str, reply: &MobileReply) -> Result<(), &'static str> {
        let mut redacted = reply.clone();
        redacted.observation = None;
        redacted.image = None;
        redacted.capabilities = None;
        sqlx::query("UPDATE mobile_operations SET reply=? WHERE owner=? AND id=?")
            .bind(serde_json::to_string(&redacted).unwrap())
            .bind(owner)
            .bind(reply.request_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|_| "storage_unavailable")?;
        let mut results = self.results.lock().await;
        if results.len() >= 64 {
            results.clear();
        }
        results.insert((owner.into(), reply.request_id), reply.clone());
        Ok(())
    }

    pub async fn poll(&self) -> Result<Value, &'static str> {
        let _guard = self.accepted.lock().await;
        loop {
            let ticket = match self.receiver.lock().await.try_recv() {
                Ok(ticket) => ticket,
                Err(_) => return Ok(Value::Null),
            };
            let mut map = self.pending.lock().await;
            let Some(job) = map.get(&ticket) else {
                continue;
            };
            let reply = self.get(&job.owner, job.id).await?;
            if job.scope.lock().await.closed.load(Ordering::Acquire) {
                let owner = job.owner.clone();
                let mut cancelled = reply;
                cancelled.state = MobileState::Cancelled;
                cancelled.error = Some("control_session_ended".into());
                self.save(&owner, &cancelled).await?;
                map.remove(&ticket);
                continue;
            }
            if reply.state != MobileState::Running {
                map.remove(&ticket);
                continue;
            }
            // Commit dispatch before handing anything to Android. A crash now is uncertain,
            // never permission to replay. The ticket is consumed exactly once.
            let mut dispatched = reply;
            dispatched.action_dispatched = job.mutation;
            sqlx::query("UPDATE mobile_operations SET claimed=1 WHERE owner=? AND id=?")
                .bind(&job.owner)
                .bind(job.id.to_string())
                .execute(&self.pool)
                .await
                .map_err(|_| "storage_unavailable")?;
            self.save(&job.owner, &dispatched).await?;
            return Ok(json!({"ticket":ticket,"command":job.payload}));
        }
    }

    pub async fn complete(
        &self,
        ticket: Uuid,
        output: Value,
        jpeg: Vec<u8>,
    ) -> Result<(), &'static str> {
        let _guard = self.accepted.lock().await;
        let Some(job) = self.pending.lock().await.remove(&ticket) else {
            return Err("expired_ticket");
        };
        let result: Result<MobileReply, &'static str> = async {
            let mut reply = MobileReply::pending(job.id);
            reply.action_dispatched = job.mutation;
            reply.state = MobileState::Completed;
            reply.platform_acknowledged = output["acknowledged"].as_bool().unwrap_or(false);
            if let Some(error) = output["error"].as_str() {
                if ![
                    "service_unavailable",
                    "control_paused",
                    "control_disconnected",
                    "screen_locked",
                    "target_stale",
                    "unsupported_action",
                    "invalid_argument",
                    "busy",
                    "platform_rejected",
                    "unconfirmed",
                    "consent_required",
                    "frame_unavailable",
                    "protected_content",
                    "condition_timeout",
                    "app_not_found",
                    "app_launch_blocked",
                ]
                .contains(&error)
                {
                    return Err("invalid_platform_result");
                }
                reply.state = if error == "unconfirmed" {
                    MobileState::Unconfirmed
                } else {
                    MobileState::Failed
                };
                // A rejection before the Android dispatch boundary must remain distinguishable.
                reply.action_dispatched = error == "unconfirmed" && job.mutation;
                reply.error = Some(error.into());
            } else if let Some(value) = output.get("capabilities") {
                let capabilities: MobileCapabilities =
                    serde_json::from_value(value.clone()).map_err(|_| "invalid_capabilities")?;
                if !capabilities.validate() {
                    return Err("invalid_capabilities");
                }
                reply.capabilities = Some(capabilities);
            } else if let Some(value) = output.get("observation") {
                let mut observation: MobileObservation =
                    serde_json::from_value(value.clone()).map_err(|_| "invalid_observation")?;
                if !observation.validate() {
                    return Err("invalid_observation");
                }
                let mut refs = job.scope.lock().await;
                refs.elements.clear();
                refs.observations.clear();
                observation.observation_ref = Uuid::new_v4().to_string();
                refs.observations
                    .insert(observation.observation_ref.clone(), observation.revision);
                for element in &mut observation.elements {
                    let issued = Uuid::new_v4().to_string();
                    refs.elements.insert(
                        issued.clone(),
                        std::mem::replace(&mut element.element_ref, issued),
                    );
                }
                reply.observation = Some(observation);
            } else if !jpeg.is_empty() {
                let width = output["width"].as_u64().ok_or("invalid_image")?;
                let height = output["height"].as_u64().ok_or("invalid_image")?;
                if jpeg.len() > 8 * 1024 * 1024
                    || !jpeg.starts_with(&[0xff, 0xd8])
                    || width == 0
                    || height == 0
                    || width
                        .checked_mul(height)
                        .is_none_or(|pixels| pixels > 16_000_000)
                {
                    return Err("invalid_image");
                }
                reply.image = Some(MobileImage {
                    width: width as u32,
                    height: height as u32,
                    size: jpeg.len() as u32,
                    sha256: format!("{:x}", Sha256::digest(&jpeg)),
                });
                *self.image.lock().await = Some((job.owner.clone(), job.id, jpeg));
            }
            Ok(reply)
        }
        .await;
        let reply = result.unwrap_or_else(|code| {
            let mut reply = MobileReply::failed(job.id, code);
            if job.mutation {
                reply.state = MobileState::Unconfirmed;
                reply.action_dispatched = true;
            }
            reply
        });
        self.save(&job.owner, &reply).await
    }

    pub async fn image(&self, owner: &str, id: RequestId) -> Option<Vec<u8>> {
        self.image
            .lock()
            .await
            .as_ref()
            .filter(|(o, i, _)| o == owner && *i == id)
            .map(|(_, _, bytes)| bytes.clone())
    }
}
