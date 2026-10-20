//! Bounded SDK storage mailbox. A dropped or expired ticket is never dispatched again.
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc, oneshot};
use uuid::Uuid;

struct Pending {
    command: Value,
    response: oneshot::Sender<Value>,
    dispatched: bool,
}

pub(crate) struct StorageBridge {
    domain: &'static str,
    pending: Mutex<HashMap<Uuid, Pending>>,
    sender: mpsc::Sender<Uuid>,
    receiver: Mutex<mpsc::Receiver<Uuid>>,
}

impl StorageBridge {
    #[cfg(test)]
    pub async fn pending_is_empty(&self) -> bool {
        self.pending.lock().await.is_empty()
    }
    pub fn new() -> Arc<Self> {
        Self::for_domain("storage")
    }
    pub fn for_domain(domain: &'static str) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel(16);
        Arc::new(Self {
            domain,
            pending: Mutex::new(HashMap::new()),
            sender,
            receiver: Mutex::new(receiver),
        })
    }

    pub async fn call(&self, mut command: Value) -> Result<Value, &'static str> {
        command["domain"] = json!(self.domain);
        let ticket = Uuid::new_v4();
        let (response, receiver) = oneshot::channel();
        self.pending.lock().await.insert(
            ticket,
            Pending {
                command,
                response,
                dispatched: false,
            },
        );
        if self.sender.try_send(ticket).is_err() {
            self.pending.lock().await.remove(&ticket);
            return Err(match self.domain {
                "system" => "system_busy",
                "document" => "document_busy",
                _ => "storage_busy",
            });
        }
        let result = tokio::time::timeout(Duration::from_secs(120), receiver).await;
        self.pending.lock().await.remove(&ticket);
        let unknown = match self.domain {
            "system" => "system_outcome_unconfirmed",
            "document" => "document_outcome_unconfirmed",
            _ => "storage_outcome_unconfirmed",
        };
        let value = result.map_err(|_| unknown)?.map_err(|_| unknown)?;
        Ok(value)
    }

    pub async fn poll(&self) -> Value {
        while let Ok(ticket) = self.receiver.lock().await.try_recv() {
            let mut pending = self.pending.lock().await;
            let Some(job) = pending.get_mut(&ticket) else {
                continue;
            };
            if job.response.is_closed() {
                pending.remove(&ticket);
                continue;
            }
            if job.dispatched {
                continue;
            }
            job.dispatched = true;
            return json!({"ticket":ticket,"command":job.command});
        }
        Value::Null
    }

    pub async fn complete(&self, ticket: Uuid, value: Value) -> bool {
        let Some(job) = self.pending.lock().await.remove(&ticket) else {
            return false;
        };
        job.dispatched && job.response.send(value).is_ok()
    }
}
