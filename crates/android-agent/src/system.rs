//! Android system/application queries use the existing wire contract and an SDK adapter.
//! Acceptance is durable before activity launch. Reconnect never replays an accepted action.
use crate::{peer::now_ms, storage::StorageBridge};
use pab_protocol::*;
use serde_json::{Value, json};
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, Notify, Semaphore};

type Key = (String, RequestId);
pub(crate) struct SystemService {
    pub sdk: Arc<StorageBridge>,
    pool: SqlitePool,
    key: [u8; 32],
    generation: String,
    device: DeviceRef,
    acceptance: Mutex<()>,
    results: Mutex<VecDeque<(Key, SystemQueryReply)>>,
    uncertain: Mutex<HashMap<Key, SystemQueryReply>>,
    slots: Arc<Semaphore>,
    capture_slot: Arc<Semaphore>,
    changed: Notify,
}

impl SystemService {
    pub fn capture_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
        self.capture_slot
            .clone()
            .try_acquire_owned()
            .map_err(|_| "screenshot_busy".into())
    }
    pub async fn capture_context(
        &self,
        owner: &str,
        scope: &str,
        id: RequestId,
        window: Option<&str>,
    ) -> Result<Value, &'static str> {
        let caller = blake3::keyed_hash(&self.key, owner.as_bytes())
            .to_hex()
            .to_string();
        let output=self.sdk.call(json!({"query":{"action":"desktop","query":{"operation":"capture","window_ref":window}},"caller":caller,"connection_scope":scope,"request_id":id,"device_ref":self.device})).await?;
        if output.get("error").is_some() {
            return Err("android_capture_context_unavailable");
        }
        Ok(output)
    }
    pub async fn open(
        path: &Path,
        key: [u8; 32],
        device: DeviceRef,
    ) -> Result<Arc<Self>, &'static str> {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(2));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .map_err(|_| "system_journal_unavailable")?;
        sqlx::query("CREATE TABLE IF NOT EXISTS system_operations(owner TEXT NOT NULL,id TEXT NOT NULL,digest TEXT NOT NULL,reply TEXT NOT NULL,PRIMARY KEY(owner,id))")
            .execute(&pool).await.map_err(|_| "system_journal_unavailable")?;
        let rows = sqlx::query("SELECT owner,id,reply FROM system_operations")
            .fetch_all(&pool)
            .await
            .map_err(|_| "system_journal_unavailable")?;
        for row in rows {
            let mut reply: SystemQueryReply =
                serde_json::from_str(row.get("reply")).map_err(|_| "system_journal_invalid")?;
            if reply.state == "running" {
                reply.state = "unconfirmed".into();
                reply.error = Some("process_restarted_outcome_unconfirmed".into());
                sqlx::query("UPDATE system_operations SET reply=? WHERE owner=? AND id=?")
                    .bind(serde_json::to_string(&reply).map_err(|_| "system_journal_invalid")?)
                    .bind(row.get::<String, _>("owner"))
                    .bind(row.get::<String, _>("id"))
                    .execute(&pool)
                    .await
                    .map_err(|_| "system_journal_unavailable")?;
            }
        }
        Ok(Arc::new(Self {
            sdk: StorageBridge::for_domain("system"),
            pool,
            key,
            generation: uuid::Uuid::new_v4().to_string(),
            device,
            acceptance: Mutex::new(()),
            results: Mutex::new(VecDeque::new()),
            uncertain: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(8)),
            capture_slot: Arc::new(Semaphore::new(1)),
            changed: Notify::new(),
        }))
    }

    fn context_ref(&self, owner: &str) -> ExecutionContextRef {
        let hash = blake3::keyed_hash(&self.key, format!("{}:{owner}", self.generation).as_bytes());
        let bytes: [u8; 16] = hash.as_bytes()[..16].try_into().expect("fixed hash prefix");
        uuid::Uuid::from_bytes(bytes)
            .to_string()
            .parse()
            .expect("UUID reference")
    }

    pub async fn request(
        self: &Arc<Self>,
        owner: &str,
        scope: &str,
        id: RequestId,
        query: SystemQuery,
        execution: ExecutionContext,
    ) -> SystemQueryReply {
        let pending = SystemQueryReply::pending(id, &query);
        if query.validate().is_err() {
            return failed(pending, "invalid_request");
        }
        let encoded = match serde_json::to_vec(&query) {
            Ok(value) => value,
            Err(_) => return failed(pending, "invalid_request"),
        };
        let digest = blake3::keyed_hash(&self.key, &encoded).to_hex().to_string();
        let permit;
        {
            let _guard = self.acceptance.lock().await;
            match sqlx::query("SELECT digest FROM system_operations WHERE owner=? AND id=?")
                .bind(owner)
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await
            {
                Ok(Some(row)) => {
                    if row.get::<String, _>("digest") != digest {
                        return failed(pending, "request_conflict");
                    }
                    return self.lookup(owner, id).await;
                }
                Err(_) => return uncertain(pending, "system_journal_unavailable"),
                _ => {}
            }
            if let SystemQuery::Applications { execution, .. } = &query
                && execution.context_ref() != Some(self.context_ref(&format!("{owner}:{scope}")))
            {
                return failed(pending, "execution_context_expired_or_wrong_caller");
            }
            permit = match self.slots.clone().try_acquire_owned() {
                Ok(value) => value,
                Err(_) => return failed(pending, "system_busy"),
            };
            let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM system_operations")
                .fetch_one(&self.pool)
                .await;
            if !matches!(count,Ok(n) if n < 10000) {
                return failed(pending, "system_journal_capacity_or_unavailable");
            }
            let stored = serde_json::to_string(&pending).expect("reply serialization");
            if sqlx::query("INSERT INTO system_operations(owner,id,digest,reply) VALUES(?,?,?,?)")
                .bind(owner)
                .bind(id.to_string())
                .bind(digest)
                .bind(stored)
                .execute(&self.pool)
                .await
                .is_err()
            {
                return uncertain(pending, "system_acceptance_unconfirmed");
            }
        }
        let service = self.clone();
        let owner = owner.to_owned();
        let lookup_owner = owner.clone();
        let scope = scope.to_owned();
        tokio::spawn(async move {
            let _permit = permit;
            let result = service.execute(&owner, &scope, id, &query, execution).await;
            // Read snapshots, especially log messages, stay in bounded transient memory.
            let mut saved = result.clone();
            if !query.is_mutation() {
                saved.data = None;
            }
            let persisted =
                sqlx::query("UPDATE system_operations SET reply=? WHERE owner=? AND id=?")
                    .bind(serde_json::to_string(&saved).expect("reply serialization"))
                    .bind(&owner)
                    .bind(id.to_string())
                    .execute(&service.pool)
                    .await;
            if persisted.is_err() {
                service.uncertain.lock().await.insert(
                    (owner, id),
                    uncertain(result, "system_outcome_persistence_unconfirmed"),
                );
            } else {
                let mut cache = service.results.lock().await;
                if cache.len() >= 64 {
                    cache.pop_front();
                }
                cache.push_back(((owner, id), result));
            }
            service.changed.notify_waiters();
        });
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let changed = self.changed.notified();
                if self.lookup(&lookup_owner, id).await.state != "running" {
                    break;
                }
                changed.await;
            }
        })
        .await;
        self.lookup(&lookup_owner, id).await
    }

    pub async fn lookup(&self, owner: &str, id: RequestId) -> SystemQueryReply {
        let key = (owner.to_owned(), id);
        if let Some(value) = self.uncertain.lock().await.get(&key) {
            return value.clone();
        }
        if let Some((_, value)) = self.results.lock().await.iter().find(|(k, _)| k == &key) {
            return value.clone();
        }
        let missing = SystemQueryReply::pending_kind(id, "system_info");
        match sqlx::query_scalar::<_, String>(
            "SELECT reply FROM system_operations WHERE owner=? AND id=?",
        )
        .bind(owner)
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        {
            Ok(Some(value)) => match serde_json::from_str::<SystemQueryReply>(&value) {
                Ok(reply) if reply.state == "completed" && reply.data.is_none() => {
                    failed(reply, "result_expired_original_not_replayed")
                }
                Ok(reply) => reply,
                Err(_) => uncertain(missing, "system_journal_invalid"),
            },
            Ok(None) => failed(missing, "not_found"),
            Err(_) => uncertain(missing, "system_journal_unavailable"),
        }
    }

    async fn execute(
        &self,
        owner: &str,
        scope: &str,
        id: RequestId,
        query: &SystemQuery,
        execution: ExecutionContext,
    ) -> SystemQueryReply {
        let mut reply = SystemQueryReply::pending(id, query);
        reply.execution_context = Some(execution);
        reply.sampled_from_unix_ms = Some(now_ms());
        let caller = blake3::keyed_hash(&self.key, owner.as_bytes())
            .to_hex()
            .to_string();
        let output = self
            .sdk
            .call(json!({"query":query,"caller":caller,"connection_scope":scope,"request_id":id,"device_ref":self.device}))
            .await;
        let mut value = match output {
            Ok(v) => v,
            Err(code) => {
                return if query.is_mutation() {
                    uncertain(reply, code)
                } else {
                    failed(reply, code)
                };
            }
        };
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return if value["action_started"].as_bool() == Some(true) {
                uncertain(reply, error)
            } else {
                failed(reply, error)
            };
        }
        if let SystemQuery::ExecutionContexts { .. } = query
            && let Some(entries) = value["data"]["entries"].as_array_mut()
        {
            for row in entries {
                if let Some(selection) = row
                    .get_mut("selection")
                    .filter(|v| v["mode"] == "desktop_user")
                {
                    selection["context_ref"] = json!(self.context_ref(&format!("{owner}:{scope}")));
                }
            }
        }
        reply.data = match serde_json::from_value(value["data"].take()) {
            Ok(data) => Some(data),
            Err(_) => {
                return if query.is_mutation() {
                    uncertain(reply, "invalid_sdk_result")
                } else {
                    failed(reply, "invalid_sdk_result")
                };
            }
        };
        if let Some(SystemQueryData::Applications { snapshot }) = &reply.data {
            let identity = match snapshot {
                AppSnapshot::List { snapshot } => Some(&snapshot.execution_identity),
                AppSnapshot::Action { result } => Some(&result.execution_identity),
                AppSnapshot::Package { result } => Some(&result.execution_identity),
                _ => None,
            };
            if let Some(identity) = identity {
                reply.execution_context.as_mut().expect("context").identity =
                    Some(identity.clone());
            }
        }
        reply.returned_count = value["count"].as_u64().unwrap_or(1).min(1000) as u32;
        reply.truncated = value["truncated"].as_bool().unwrap_or(false);
        reply.stop_reason = reply.truncated.then(|| "entry_or_byte_limit".into());
        reply.warnings = serde_json::from_value(value["warnings"].take()).unwrap_or_default();
        reply.warnings.truncate(8);
        reply.sampled_at_unix_ms = Some(now_ms());
        reply.state = "completed".into();
        if !serde_json::to_vec(&reply).is_ok_and(|v| v.len() <= MAX_SYSTEM_REPLY_BYTES) {
            return if query.is_mutation() {
                uncertain(reply, "reply_byte_limit")
            } else {
                failed(reply, "reply_byte_limit")
            };
        }
        reply
    }
}
fn failed(mut reply: SystemQueryReply, code: &str) -> SystemQueryReply {
    reply.state = "failed".into();
    reply.error = Some(code.into());
    reply.data = None;
    reply.returned_count = 0;
    reply
}
fn uncertain(mut reply: SystemQueryReply, code: &str) -> SystemQueryReply {
    reply.state = "unconfirmed".into();
    reply.error = Some(code.into());
    reply.data = None;
    reply.returned_count = 0;
    reply
}

#[cfg(test)]
#[path = "system_tests.rs"]
mod tests;
