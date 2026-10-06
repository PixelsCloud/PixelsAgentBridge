use super::*;
use crate::{
    ui_engine::UiWindow,
    ui_registry::UiOwner,
    ui_worker::WorkerProcess,
    ui_worker_entry::{WorkerCommand, WorkerMessage, WorkerReply},
};
use std::{process::Command, time::Duration};

impl DesktopSession {
    pub(crate) fn ui_query(
        &mut self,
        id: RequestId,
        query: &UiRequest,
        context: Option<UiHelperContext>,
        guard: &mut (impl FnMut() -> Result<(), String> + Send),
    ) -> SystemQueryReply {
        let system = SystemQuery::Desktop {
            query: DesktopQuery::Ui {
                query: query.clone(),
            },
        };
        let mut reply = SystemQueryReply::pending(id, &system);
        let start = now();
        reply.sampled_from_unix_ms = Some(start);
        let mut snapshot = DesktopSnapshot::new(self.instance.clone(), "isolated_accessibility");
        let mut sent = false;
        let operation = (|| -> Result<UiSnapshot, String> {
            query.validate().map_err(str::to_owned)?;
            let context = context.ok_or("authenticated UI connection context missing")?;
            if !cfg!(any(windows, target_os = "macos")) {
                return Err("unsupported_platform".into());
            }
            guard()?;
            check_session()?;
            let scope = match query {
                UiRequest::Query { scope, .. } | UiRequest::Wait { scope, .. } => Some(scope),
                _ => None,
            };
            let ticket = if let Some(UiScope::Window { window_ref }) = scope {
                let e = self.windows.get(window_ref).ok_or("stale_window")?;
                if pab_os_control::process_identity(e.pid)? != e.process_identity {
                    return Err("stale_window".into());
                }
                Some(UiWindow {
                    window_ref: window_ref.clone(),
                    id: e.id,
                    pid: e.pid,
                    process_identity: e.process_identity.clone(),
                    marker_key: self.key.clone(),
                    marker: e.marker,
                })
            } else {
                None
            };
            if self.ui_worker.is_none() {
                if ticket.is_none() {
                    return Err("stale_element: worker restarted; query the window again".into());
                }
                let mut command = Command::new(
                    std::env::current_exe().map_err(|_| "worker executable unavailable")?,
                );
                command.arg("--ui-worker");
                self.ui_worker =
                    Some(WorkerProcess::spawn(&mut command).map_err(|_| "worker unavailable")?);
            }
            let command = match query {
                UiRequest::Wait {
                    scope,
                    selector,
                    condition,
                    ..
                } if context.sample => WorkerCommand::Sample {
                    ticket,
                    scope: scope.clone(),
                    selector: selector.clone(),
                    condition: condition.clone(),
                },
                UiRequest::Wait { .. } => return Err("wait_requires_executor_sampling".into()),
                _ if context.sample => return Err("invalid UI sample context".into()),
                _ => WorkerCommand::Query {
                    ticket,
                    request: query.clone(),
                },
            };
            let timeout = match query {
                UiRequest::Query { limits, .. } => limits.timeout_ms,
                UiRequest::Action { timeout_ms, .. } => *timeout_ms,
                UiRequest::Wait { timeout_ms, .. } => (*timeout_ms).min(3000),
                _ => 3000,
            };
            let message = WorkerMessage {
                request_id: id,
                owner: UiOwner {
                    connection: context.connection_id,
                    helper_instance: self.instance.clone(),
                },
                command,
            };
            let value = serde_json::to_value(message).map_err(|_| "worker request invalid")?;
            sent = true;
            let response = self
                .ui_worker
                .as_mut()
                .unwrap()
                .exchange(&value, Duration::from_millis(timeout as u64));
            let value = match response {
                Ok(value) => value,
                Err(error) => {
                    self.ui_worker.take();
                    return Err(format!(
                        "ui_worker_{error:?}: references invalidated; no automatic replay"
                    ));
                }
            };
            let response: WorkerReply = match serde_json::from_value(value) {
                Ok(response) => response,
                Err(_) => {
                    self.ui_worker.take();
                    return Err("worker reply invalid".into());
                }
            };
            if response.request_id != id {
                self.ui_worker.take();
                return Err("worker reply identity mismatch".into());
            }
            let Some(snapshot) = response.snapshot else {
                self.ui_worker.take();
                return Err("worker snapshot missing".into());
            };
            if snapshot.validate().is_err() {
                self.ui_worker.take();
                return Err("worker snapshot invalid".into());
            }
            Ok(snapshot)
        })();
        let mut ui = match operation {
            Ok(ui) => ui,
            Err(error) => UiSnapshot {
                elements: vec![],
                visited_count: 0,
                truncated: false,
                stop_reason: None,
                outcome: if sent && query.is_mutation() {
                    UiOutcome::Unconfirmed
                } else {
                    UiOutcome::Rejected
                },
                action_dispatched: if sent && query.is_mutation() {
                    None
                } else {
                    Some(false)
                },
                verification: UiVerification::Unavailable,
                error_code: Some(short(&error, 256)),
                sampled_from_unix_ms: start,
                sampled_at_unix_ms: now(),
            },
        };
        if let Err(error) = guard().and_then(|_| check_session()) {
            ui.outcome = if ui.action_dispatched != Some(false) {
                UiOutcome::Unconfirmed
            } else {
                UiOutcome::Rejected
            };
            ui.error_code = Some(short(&error, 256));
            self.ui_worker.take();
        }
        reply.state = match ui.outcome {
            UiOutcome::Unconfirmed => "unconfirmed",
            UiOutcome::Rejected => "failed",
            UiOutcome::Cancelled => "cancelled",
            _ => "completed",
        }
        .into();
        reply.returned_count = ui.elements.len() as u32;
        reply.truncated = ui.truncated;
        reply.stop_reason = ui.stop_reason.clone();
        reply.error = ui.error_code.clone();
        reply.sampled_at_unix_ms = Some(now());
        snapshot.action_started = ui.action_dispatched != Some(false);
        snapshot.ui = Some(ui);
        reply.data = Some(SystemQueryData::Desktop { snapshot });
        reply
    }

    pub fn release_ui_connection(&mut self, connection: RequestId) {
        let Some(worker) = self.ui_worker.as_mut() else {
            return;
        };
        let message = WorkerMessage {
            request_id: RequestId::new(),
            owner: UiOwner {
                connection,
                helper_instance: self.instance.clone(),
            },
            command: WorkerCommand::ReleaseOwner,
        };
        if serde_json::to_value(message)
            .ok()
            .and_then(|v| worker.exchange(&v, Duration::from_secs(1)).ok())
            .is_none()
        {
            self.ui_worker.take();
        }
    }
    pub(crate) fn release_ui_window(&mut self, window_ref: String) {
        let Some(worker) = self.ui_worker.as_mut() else {
            return;
        };
        let message = WorkerMessage {
            request_id: RequestId::new(),
            owner: UiOwner {
                connection: RequestId::new(),
                helper_instance: self.instance.clone(),
            },
            command: WorkerCommand::ReleaseWindow { window_ref },
        };
        if serde_json::to_value(message)
            .ok()
            .and_then(|v| worker.exchange(&v, Duration::from_secs(1)).ok())
            .is_none()
        {
            self.ui_worker.take();
        }
    }
}
