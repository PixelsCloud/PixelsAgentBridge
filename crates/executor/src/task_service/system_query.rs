use super::{TaskService, TaskServiceError};
use pab_protocol::{OperatorRef, RequestId, SystemQuery, SystemQueryReply};

impl TaskService {
    pub(crate) async fn system_query(
        &self,
        actor: OperatorRef,
        id: RequestId,
        query: SystemQuery,
    ) -> Result<SystemQueryReply, TaskServiceError> {
        query.validate().map_err(TaskServiceError::InvalidRequest)?;
        // Serialize acceptance/registration to keep a duplicate from observing a missing worker.
        let mut jobs = self.system_jobs.lock().await;
        if !self.store.accept_system_query(actor, id, &query).await? {
            drop(jobs);
            return self.get_system_query(actor, id).await;
        }
        let permit = match self.system_pending_slots.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                let mut r = SystemQueryReply::pending(id, &query);
                r.state = "failed".into();
                r.error =
                    Some("executor_busy: system query queue is full (16 accepted queries)".into());
                self.store.finish_system_query(&r).await?;
                return Ok(r);
            }
        };
        jobs.insert(id);
        self.system_queued.lock().await.insert(id);
        let (git_cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let cancellable = matches!(
            query,
            SystemQuery::Git { .. }
                | SystemQuery::Container { .. }
                | SystemQuery::Desktop {
                    query: pab_protocol::DesktopQuery::Ui { .. }
                }
        );
        if cancellable {
            self.cancellable_system_jobs
                .lock()
                .await
                .insert(id, git_cancel);
        }
        drop(jobs);
        let mutation = query.is_mutation();
        let ui_query = matches!(
            query,
            SystemQuery::Desktop {
                query: pab_protocol::DesktopQuery::Ui { .. }
            }
        );
        let service = self.clone();
        let collector = self.system_collector.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _permit = permit;
            let active_wait = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                service.system_slots.clone().acquire_owned(),
            );
            let active = tokio::select! {
                biased;
                _ = cancelled.wait_for(|value| *value), if cancellable => None,
                result = active_wait => Some(result),
            };
            service.system_queued.lock().await.remove(&id);
            let _active = match active {
                Some(Ok(Ok(p))) if !*cancelled.borrow() => p,
                _ => {
                    let mut r = SystemQueryReply::pending(id, &query);
                    if *cancelled.borrow() {
                        r.state = "cancelled".into();
                        r.error = Some("cancelled before execution".into());
                    } else {
                        r.state = "failed".into();
                        r.error =
                            Some("queue_timeout: system query did not start within 5000 ms".into());
                    }
                    if let Err(e) = service.store.finish_system_query(&r).await {
                        tracing::warn!(%id, %e, "queued system query result persistence failed");
                    }
                    service.system_jobs.lock().await.remove(&id);
                    service.cancellable_system_jobs.lock().await.remove(&id);
                    let _ = send.send(());
                    return;
                }
            };
            // A wait must not occupy the two system collectors and prevent the
            // action that could satisfy it from being scheduled.
            let _active = if matches!(
                query,
                SystemQuery::Desktop {
                    query: pab_protocol::DesktopQuery::Ui {
                        query: pab_protocol::UiRequest::Wait { .. }
                    }
                }
            ) {
                drop(_active);
                None
            } else {
                Some(_active)
            };
            let fallback = query.clone();
            let async_query = query.clone();
            let async_result =
                tokio::spawn(async move { pab_platform::query_async(id, &async_query).await })
                    .await;
            let r = if matches!(query, SystemQuery::ExecutionContexts { .. }) {
                let mut reply = pab_platform::collect_execution_contexts(id, &query).await;
                if let Some(pab_protocol::SystemQueryData::ExecutionContexts { entries, .. }) =
                    &mut reply.data
                {
                    let caller = pab_task_runtime::ExecutionCaller {
                        device: service.device_ref,
                        actor,
                        connection: service.ui_connection.id,
                    };
                    let mut registry = service.ui_connection.execution_contexts.lock().await;
                    for entry in entries {
                        let Some(identity) = entry.identity.as_ref() else {
                            continue;
                        };
                        if identity.mode == pab_protocol::ExecutionMode::Service {
                            entry.selection = Some(pab_protocol::ExecutionSelection::Service {});
                        } else {
                            match registry.register(caller, identity.clone()) {
                                Ok(context_ref) => {
                                    entry.selection =
                                        Some(pab_protocol::ExecutionSelection::User { context_ref })
                                }
                                Err(error) => entry.unavailable_reason = Some(error.to_string()),
                            }
                        }
                    }
                }
                pab_platform::bound_system_reply(&mut reply);
                reply
            } else if let SystemQuery::Desktop {
                query: pab_protocol::DesktopQuery::Ui { query: ui },
            } = &query
            {
                service.ui_query(id, ui, cancelled).await
            } else if let SystemQuery::Container { query: container } = &query {
                {
                    #[cfg(test)]
                    if let Some((docker, endpoint)) = &service.container_client {
                        super::container::query_with_client(
                            id,
                            container,
                            cancelled,
                            Some(service.store.clone()),
                            docker.clone(),
                            endpoint.clone(),
                        )
                        .await
                    } else {
                        super::container::query(
                            id,
                            container,
                            cancelled,
                            Some(service.store.clone()),
                        )
                        .await
                    }
                    #[cfg(not(test))]
                    super::container::query(id, container, cancelled, Some(service.store.clone()))
                        .await
                }
            } else if let SystemQuery::Git { query: git } = &query {
                super::git::query(id, git, cancelled, Some(service.store.clone())).await
            } else if let SystemQuery::Desktop { query: desktop } = &query {
                match crate::local_ipc::request_desktop_query(id, desktop.clone()).await {
                    Ok(r) => r,
                    Err(error) => {
                        let mut r = SystemQueryReply::pending(id, &query);
                        r.state = if query.is_mutation()
                            && !matches!(
                                error,
                                crate::local_ipc::LocalIpcError::WindowHelperUnavailable
                            ) {
                            "unconfirmed"
                        } else {
                            "failed"
                        }
                        .into();
                        r.error = Some(error.to_string());
                        r
                    }
                }
            } else {
                match async_result {
                    Ok(Some(reply)) => reply,
                    Ok(None) => {
                        service.system_queued.lock().await.insert(id);
                        let guard = tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            collector.lock_owned(),
                        )
                        .await;
                        service.system_queued.lock().await.remove(&id);
                        let result = match guard {
                            Ok(mut c) => {
                                tokio::task::spawn_blocking(move || c.query(id, &query)).await
                            }
                            Err(_) => {
                                let mut r = SystemQueryReply::pending(id, &query);
                                r.state = "failed".into();
                                r.error = Some("queue_timeout: system collector did not become available within 5000 ms".into());
                                Ok(r)
                            }
                        };
                        result.unwrap_or_else(|_| {
                            let mut r = SystemQueryReply::pending(id, &fallback);
                            r.state = "failed".into();
                            r.error = Some("system collector stopped unexpectedly".into());
                            r
                        })
                    }
                    Err(_) => {
                        let mut r = SystemQueryReply::pending(id, &fallback);
                        r.state = "failed".into();
                        r.error = Some("async query worker stopped unexpectedly".into());
                        r
                    }
                }
            };
            if let Err(e) = service.store.finish_system_query(&r).await {
                tracing::warn!(%id,%e,"system query result persistence failed");
            }
            service.system_jobs.lock().await.remove(&id);
            service.cancellable_system_jobs.lock().await.remove(&id);
            let _ = send.send(());
        });
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(if mutation || ui_query { 250 } else { 5000 }),
            receive,
        )
        .await;
        self.get_system_query(actor, id).await
    }
    pub(crate) async fn get_system_query(
        &self,
        actor: OperatorRef,
        id: RequestId,
    ) -> Result<SystemQueryReply, TaskServiceError> {
        let mut r = self.store.get_system_query(actor, id).await?;
        if r.state == "running" && self.system_queued.lock().await.contains(&id) {
            r.warnings
                .push("queued: waiting for a system query execution slot".into());
        }
        if r.state == "running" && !self.system_jobs.lock().await.contains(&id) {
            r = self.store.get_system_query(actor, id).await?;
            if r.state == "running" {
                r.state = "unconfirmed".into();
                r.error = Some("worker unavailable; original result cannot be confirmed".into());
            }
        }
        if r.state == "unconfirmed"
            && r.kind == "git_push"
            && !self.system_jobs.lock().await.contains(&id)
            && let SystemQuery::Git { query } = self.store.system_query_spec(actor, id).await?
            && let Some(updated) = super::git::reconcile_push(&query, &r).await
        {
            self.store.finish_system_query(&updated).await?;
            r = updated;
        }
        if r.state == "unconfirmed"
            && r.kind == "container_control"
            && !self.system_jobs.lock().await.contains(&id)
            && let SystemQuery::Container { query } =
                self.store.system_query_spec(actor, id).await?
            && let Some(updated) = super::container::reconcile(&query, &r).await
        {
            self.store.finish_system_query(&updated).await?;
            r = updated;
        }
        Ok(r)
    }
    pub(crate) async fn cancel_system_query(
        &self,
        actor: OperatorRef,
        id: RequestId,
    ) -> Result<SystemQueryReply, TaskServiceError> {
        let mut r = self.store.get_system_query(actor, id).await?;
        if !r.kind.starts_with("git_")
            && !["ui_query", "ui_get", "ui_action", "ui_wait"].contains(&r.kind.as_str())
            && ![
                "containers",
                "container",
                "container_logs",
                "container_control",
            ]
            .contains(&r.kind.as_str())
        {
            return Err(TaskServiceError::InvalidRequest(
                "this system operation cannot be cancelled",
            ));
        }
        if r.state != "running" {
            return Ok(r);
        }
        if let Some(cancel) = self.cancellable_system_jobs.lock().await.get(&id) {
            let _ = cancel.send(true);
            r.state = "cancel_requested".into();
            r.error = Some(
                "cancellation requested; already dispatched effects are not rolled back".into(),
            );
        }
        Ok(r)
    }
}

#[cfg(test)]
#[path = "system_query_tests.rs"]
mod tests;
