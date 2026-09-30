use super::{TaskService, TaskServiceError};
use pab_protocol::{OperatorRef, RequestId, SystemQuery, SystemQueryReply};

impl TaskService {
    pub(super) async fn system_query(
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
        let permit = match self.system_slots.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                let mut r = SystemQueryReply::pending(id, &query);
                r.state = "failed".into();
                r.error = Some("executor_busy: two system queries are active".into());
                self.store.finish_system_query(&r).await?;
                return Ok(r);
            }
        };
        jobs.insert(id);
        drop(jobs);
        let service = self.clone();
        let collector = self.system_collector.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _permit = permit;
            let fallback = query.clone();
            let result = tokio::task::spawn_blocking(move || match collector.try_lock() {
                Ok(mut c) => c.query(id, &query),
                Err(_) => {
                    let mut r = SystemQueryReply::pending(id, &query);
                    r.state = "failed".into();
                    r.error = Some("executor_busy: system collector unavailable".into());
                    r
                }
            })
            .await;
            let r = result.unwrap_or_else(|_| {
                let mut r = SystemQueryReply::pending(id, &fallback);
                r.state = "failed".into();
                r.error = Some("system collector stopped unexpectedly".into());
                r
            });
            if let Err(e) = service.store.finish_system_query(&r).await {
                tracing::warn!(%id,%e,"system query result persistence failed");
            }
            service.system_jobs.lock().await.remove(&id);
            let _ = send.send(());
        });
        let _ = receive.await;
        self.get_system_query(actor, id).await
    }
    pub(super) async fn get_system_query(
        &self,
        actor: OperatorRef,
        id: RequestId,
    ) -> Result<SystemQueryReply, TaskServiceError> {
        let mut r = self.store.get_system_query(actor, id).await?;
        if r.state == "running" && !self.system_jobs.lock().await.contains(&id) {
            r = self.store.get_system_query(actor, id).await?;
            if r.state == "running" {
                r.state = "unconfirmed".into();
                r.error = Some("worker unavailable; original result cannot be confirmed".into());
            }
        }
        Ok(r)
    }
}

#[cfg(test)]
#[path = "system_query_tests.rs"]
mod tests;
