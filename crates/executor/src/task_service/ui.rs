use super::TaskService;
use pab_protocol::*;
use std::time::Duration;
use tokio::sync::watch;

impl TaskService {
    pub(super) async fn ui_query(
        &self,
        id: RequestId,
        ui: &UiRequest,
        mut cancelled: watch::Receiver<bool>,
    ) -> SystemQueryReply {
        self.ui_connection
            .used
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let query = SystemQuery::Desktop {
            query: DesktopQuery::Ui { query: ui.clone() },
        };
        let wait = matches!(ui, UiRequest::Wait { .. });
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(match ui {
                UiRequest::Wait { timeout_ms, .. } => *timeout_ms as u64,
                _ => 0,
            });
        let mut last = SystemQueryReply::pending(id, &query);
        loop {
            if *cancelled.borrow() {
                return finish(last, "cancelled", UiOutcome::Cancelled);
            }
            let mut sample = ui.clone();
            if let UiRequest::Wait {
                timeout_ms,
                poll_ms,
                ..
            } = &mut sample
            {
                let remaining = deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .as_millis();
                if remaining == 0 {
                    return finish(last, "completed", UiOutcome::TimedOut);
                }
                if remaining < 100 && last.data.is_some() {
                    tokio::select! {
                        _=cancelled.wait_for(|v|*v)=>return finish(last,"cancelled",UiOutcome::Cancelled),
                        _=tokio::time::sleep_until(deadline)=>return finish(last,"completed",UiOutcome::TimedOut),
                    }
                }
                *timeout_ms = remaining.clamp(100, 3000) as u32;
                *poll_ms = (*poll_ms).min(*timeout_ms);
            }
            last = match crate::local_ipc::request_desktop_query_with_context(
                id,
                DesktopQuery::Ui { query: sample },
                Some(UiHelperContext {
                    connection_id: self.ui_connection.id,
                    sample: wait,
                }),
            )
            .await
            {
                Ok(reply) => reply,
                Err(error) => {
                    let mut reply = SystemQueryReply::pending(id, &query);
                    reply.state = if ui.is_mutation()
                        && !matches!(
                            error,
                            crate::local_ipc::LocalIpcError::WindowHelperUnavailable
                        ) {
                        "unconfirmed"
                    } else {
                        "failed"
                    }
                    .into();
                    reply.error = Some(error.to_string());
                    return reply;
                }
            };
            if !wait || last.error.is_some() {
                return last;
            }
            let matched = matches!(&last.data,Some(SystemQueryData::Desktop {snapshot}) if snapshot.ui.as_ref().is_some_and(|u|u.outcome==UiOutcome::Matched));
            if matched {
                return last;
            }
            if tokio::time::Instant::now() >= deadline {
                return finish(last, "completed", UiOutcome::TimedOut);
            }
            let poll = match ui {
                UiRequest::Wait { poll_ms, .. } => *poll_ms,
                _ => 250,
            };
            tokio::select! {
                _=cancelled.wait_for(|v|*v)=>return finish(last,"cancelled",UiOutcome::Cancelled),
                _=tokio::time::sleep_until((tokio::time::Instant::now()+Duration::from_millis(poll.into())).min(deadline))=>{}
            }
        }
    }
}
fn finish(mut reply: SystemQueryReply, state: &str, outcome: UiOutcome) -> SystemQueryReply {
    reply.state = state.into();
    if reply.data.is_none() {
        let now = super::unix_millis().unwrap_or_default();
        let mut snapshot = DesktopSnapshot::new(String::new(), "isolated_accessibility");
        snapshot.ui = Some(UiSnapshot {
            elements: vec![],
            visited_count: 0,
            truncated: false,
            stop_reason: None,
            outcome,
            action_dispatched: Some(false),
            verification: UiVerification::NotApplicable,
            error_code: None,
            sampled_from_unix_ms: now,
            sampled_at_unix_ms: now,
        });
        reply.data = Some(SystemQueryData::Desktop { snapshot });
        reply.sampled_at_unix_ms = Some(now);
    }
    if let Some(SystemQueryData::Desktop { snapshot }) = &mut reply.data {
        if let Some(ui) = &mut snapshot.ui {
            ui.outcome = outcome;
        }
    }
    reply
}
