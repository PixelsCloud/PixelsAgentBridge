use pab_bridge::desktop_assistance::{self, Request, Snapshot, Store};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Arguments {
    Request {
        request_id: Uuid,
        device_code: pab_protocol::DeviceCode,
        reason: String,
        expected_action: String,
        screen_id: Option<String>,
        #[serde(default = "expiry")]
        expires_in_seconds: u32,
    },
    Status {
        assistance_id: Uuid,
    },
    Wait {
        assistance_id: Uuid,
        after_revision: Option<u64>,
        #[serde(default = "wait_time")]
        wait_ms: u64,
    },
    Cancel {
        assistance_id: Uuid,
    },
}
fn expiry() -> u32 {
    900
}
fn wait_time() -> u64 {
    30000
}
pub(super) fn parse(args: &Value) -> Result<Arguments, String> {
    let result: Arguments = serde_json::from_value(args.clone())
        .map_err(|_| "invalid desktop assistance arguments".to_string())?;
    if let Arguments::Wait { wait_ms, .. } = &result {
        if *wait_ms > 30000 {
            return Err("wait_ms must be at most 30000".into());
        }
    }
    if let Arguments::Request {
        request_id,
        device_code,
        reason,
        expected_action,
        screen_id,
        expires_in_seconds,
    } = &result
    {
        Request {
            request_id: *request_id,
            device_code: *device_code,
            reason: reason.clone(),
            expected_action: expected_action.clone(),
            screen_id: screen_id.clone(),
            expires_in_seconds: *expires_in_seconds,
        }
        .validate()
        .map_err(|e| e.to_string())?;
    }
    Ok(result)
}
pub(super) fn definition() -> Value {
    json!({"name":"pab_desktop_assistance","description":"Ask the user to help on a connected remote desktop. Uses this OS user's Desktop and existing signed-in account/service; does not send requests over the read-only MCP reporting socket. request requires a stable request_id, reason and expected_action; never include passwords, codes or typed contents. Reuse request_id after an uncertain response. The reply separates waiting/active/completed/cancelled/expired from delivery desktop_not_running/pending/displayed. Only the user's explicit completion counts as completed; re-check the actual result before resuming. Cancel affects this request, not an existing viewer. status/wait/cancel belong to this MCP session; wait is renewable and does not cancel on timeout.","inputSchema":{
        "type":"object","properties":{
            "action":{"type":"string","enum":["request","status","wait","cancel"]},
            "request_id":{"type":"string","format":"uuid"},"assistance_id":{"type":"string","format":"uuid"},
            "device_code":{"type":"string","pattern":"^[0-9]{9}$"},
            "reason":{"type":"string","minLength":1,"maxLength":4096},
            "expected_action":{"type":"string","minLength":1,"maxLength":4096},
            "screen_id":{"type":"string","minLength":1,"maxLength":1024},
            "expires_in_seconds":{"type":"integer","minimum":1,"maximum":86400,"default":900},
            "after_revision":{"type":"integer","minimum":0},
            "wait_ms":{"type":"integer","minimum":0,"maximum":30000,"default":30000}
        },"required":["action"],"additionalProperties":false,
        "oneOf":[
            {"properties":{"action":{"const":"request"}},"required":["request_id","device_code","reason","expected_action"]},
            {"properties":{"action":{"enum":["status","wait","cancel"]}},"required":["assistance_id"]}
        ]
    }})
}

#[derive(Default)]
pub(super) struct Manager(Mutex<Option<Arc<Session>>>);
struct Session {
    store: Store,
    owner: Uuid,
    heartbeat: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(task) = self.heartbeat.get_mut() {
            if let Some(task) = task.take() {
                task.abort();
            }
        }
    }
}
impl Session {
    async fn stop(&self) -> Result<(), String> {
        let task = self
            .heartbeat
            .lock()
            .map_err(|_| "assistance heartbeat lock failed")?
            .take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        self.store
            .stop_requester(self.owner)
            .await
            .map_err(|e| e.to_string())
    }
}
impl Manager {
    async fn session(&self) -> Result<Arc<Session>, String> {
        let mut current = self.0.lock().await;
        if current
            .as_ref()
            .is_some_and(|s| !s.store.is_current_account())
        {
            let old = current.take().unwrap();
            old.stop().await?;
        }
        if current.is_none() {
            let store = Store::for_current_account()
                .await
                .map_err(|e| e.to_string())?;
            let owner = Uuid::new_v4();
            store
                .heartbeat_requester(owner)
                .await
                .map_err(|e| e.to_string())?;
            let live = store.clone();
            let heartbeat = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(desktop_assistance::HEARTBEAT).await;
                    if !live.is_current_account() || live.heartbeat_requester(owner).await.is_err()
                    {
                        break;
                    }
                }
                let _ = live.stop_requester(owner).await;
            });
            *current = Some(Arc::new(Session {
                store,
                owner,
                heartbeat: std::sync::Mutex::new(Some(heartbeat)),
            }));
        }
        Ok(current.as_ref().unwrap().clone())
    }
    pub(super) async fn shutdown(&self) {
        if let Some(session) = self.0.lock().await.take() {
            let _ = session.stop().await;
        }
    }
    pub(super) async fn call(
        &self,
        args: Arguments,
        server: &super::McpServer,
    ) -> Result<Value, String> {
        let session = self.session().await?;
        let snapshot = match args {
            Arguments::Request {
                request_id,
                device_code,
                reason,
                expected_action,
                screen_id,
                expires_in_seconds,
            } => {
                let request = Request {
                    request_id,
                    device_code,
                    reason,
                    expected_action,
                    screen_id,
                    expires_in_seconds,
                };
                if let Some(snapshot) = session
                    .store
                    .replay(session.owner, &request)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    if !session.store.is_current_account() {
                        return Err("account changed; assistance session ended".into());
                    }
                    return response(snapshot);
                }
                let manager = server.operation_manager().await?;
                let runtime = manager.runtime().await?;
                let target =
                    super::mcp_tools::resolve_target(&runtime, &json!({"device_code":device_code}))
                        .await?;
                runtime
                    .connect_device(target)
                    .await
                    .map_err(|e| e.to_string())?;
                if !session.store.is_current_account() {
                    return Err("account changed; start a new assistance request".into());
                }
                session
                    .store
                    .request(session.owner, request)
                    .await
                    .map_err(|e| e.to_string())?
            }
            Arguments::Status { assistance_id } => session
                .store
                .status(session.owner, assistance_id)
                .await
                .map_err(|e| e.to_string())?,
            Arguments::Cancel { assistance_id } => session
                .store
                .cancel(session.owner, assistance_id)
                .await
                .map_err(|e| e.to_string())?,
            Arguments::Wait {
                assistance_id,
                after_revision,
                wait_ms,
            } => {
                let until = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
                loop {
                    if !session.store.is_current_account() {
                        return Err("account changed; assistance session ended".into());
                    }
                    let value = session
                        .store
                        .status(session.owner, assistance_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    if value.status.terminal()
                        || after_revision.is_some_and(|r| value.revision != r)
                        || tokio::time::Instant::now() >= until
                    {
                        break value;
                    }
                    tokio::time::sleep_until(
                        until.min(tokio::time::Instant::now() + Duration::from_millis(200)),
                    )
                    .await;
                }
            }
        };
        if !session.store.is_current_account() {
            return Err("account changed; assistance session ended".into());
        }
        response(snapshot)
    }
}
fn response(snapshot: Snapshot) -> Result<Value, String> {
    let terminal = snapshot.status.terminal();
    Ok(json!({"assistance":snapshot,"terminal":terminal,"verify_result_before_resuming":true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn action_fields_are_exclusive_and_wait_is_bounded() {
        assert!(parse(&json!({"action":"status","assistance_id":Uuid::new_v4()})).is_ok());
        for args in [
            json!({"action":"status","assistance_id":Uuid::new_v4(),"device_code":"123456789"}),
            json!({"action":"wait","assistance_id":Uuid::new_v4(),"wait_ms":30001}),
            json!({"action":"request","device_code":"123456789"}),
        ] {
            assert!(parse(&args).is_err());
        }
    }
}
