use crate::{RelayControlClient, RelayPolicyRuntime};
use pab_agent_core::usage_spool::UsageSpool;
use pab_protocol::{UsageBatch, UsageCounters};
use std::{path::PathBuf, time::Duration};

pub(crate) struct UsageWorker {
    stop: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}
impl UsageWorker {
    pub async fn shutdown(mut self) {
        let _ = self.stop.send(true);
        if tokio::time::timeout(Duration::from_secs(15), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
            let _ = self.task.await;
        }
    }
}
pub(crate) fn spawn(
    runtime: RelayPolicyRuntime,
    url: String,
    secret: String,
    connector: tokio_tungstenite::Connector,
) -> UsageWorker {
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        let node = std::env::var("PAB_RELAY_NODE_ID").unwrap_or_else(|_| "primary".into());
        let root = std::env::var_os("PAB_RELAY_USAGE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("relay-usage"));
        let scope = format!("{url}\n{node}");
        let mut client = None;
        let mut backlog = Vec::<UsageBatch>::new();
        let mut marked = false;
        loop {
            let spool = match UsageSpool::open(&root.join("usage.sqlite3")).await {
                Ok(v) => v,
                Err(_) => {
                    tracing::warn!("Relay usage spool unavailable; retrying");
                    if *stopped.borrow() {
                        return;
                    }
                    tokio::select! {_=tokio::time::sleep(Duration::from_secs(5))=>{},_=stopped.changed()=>{}}
                    continue;
                }
            };
            if !marked {
                let marker = root.join("active");
                if marker.exists() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;
                    backlog.push(UsageBatch {
                        id: uuid::Uuid::new_v4(),
                        user_id: None,
                        hour_unix_ms: now - now % 3_600_000,
                        counters: UsageCounters {
                            incomplete: true,
                            ..Default::default()
                        },
                    });
                }
                // An interrupted producer may have lost its final in-memory window.
                let _ = std::fs::write(marker, b"usage may contain an interrupted window\n");
                marked = true;
            }
            if backlog.is_empty() {
                if let Ok(mut counters) = runtime.usage.lock() {
                    for ((user_id, hour_unix_ms), counters) in counters.drain() {
                        backlog.push(UsageBatch {
                            id: uuid::Uuid::new_v4(),
                            user_id,
                            hour_unix_ms,
                            counters,
                        });
                    }
                }
            }
            while let Some(batch) = backlog.last() {
                if spool.push(&scope, batch).await.is_err() {
                    break;
                }
                backlog.pop();
            }
            if *stopped.borrow() {
                // Forwarding has stopped. Persist any counters accumulated while a previous backlog was draining.
                if backlog.is_empty() {
                    if let Ok(mut counters) = runtime.usage.lock() {
                        for ((user_id, hour_unix_ms), counters) in counters.drain() {
                            backlog.push(UsageBatch {
                                id: uuid::Uuid::new_v4(),
                                user_id,
                                hour_unix_ms,
                                counters,
                            });
                        }
                    }
                }
                while let Some(batch) = backlog.last() {
                    if spool.push(&scope, batch).await.is_err() {
                        break;
                    }
                    backlog.pop();
                }
                if backlog.is_empty() {
                    let _ = std::fs::remove_file(root.join("active"));
                } else {
                    tracing::warn!("Relay stopped with an unpersisted usage window");
                }
                return;
            }
            if client.is_none() {
                client = RelayControlClient::connect(&url, &secret, connector.clone())
                    .await
                    .ok();
            }
            if let Some(connection) = client.as_mut() {
                if let Ok(batches) = spool.pending(&scope).await {
                    for batch in batches {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64;
                        if !batch.valid_at(now) {
                            let _ = spool.acknowledge(batch.id).await;
                            if let Ok(mut usage) = runtime.usage.lock() {
                                usage
                                    .entry((batch.user_id, now - now % 3_600_000))
                                    .or_default()
                                    .incomplete = true;
                            }
                            continue;
                        }
                        if connection.report_usage(&node, &batch).await.is_err() {
                            client = None;
                            break;
                        }
                        if spool.acknowledge(batch.id).await.is_err() {
                            break;
                        }
                    }
                }
            }
            tokio::select! {_=tokio::time::sleep(Duration::from_secs(5))=>{},_=stopped.changed()=>{}}
        }
    });
    UsageWorker { stop, task }
}
