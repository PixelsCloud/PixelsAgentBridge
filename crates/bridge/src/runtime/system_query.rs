use super::{
    BridgeRuntime, RuntimeError, RuntimeStore, RuntimeStoreError, TransferQueue,
    operation::now_unix_ms,
};
use pab_protocol::*;
use sqlx::Row;

impl RuntimeStore {
    async fn owns_system_query(
        &self,
        device: DeviceRef,
        id: RequestId,
        actor: &str,
        owner: &str,
    ) -> Result<bool, RuntimeStoreError> {
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM runtime_operations WHERE id=? AND device_ref_json=? AND initiated_by=? AND owner_session_id=?")
            .bind(id.to_string()).bind(serde_json::to_string(&device)?).bind(actor).bind(owner).fetch_one(&self.pool).await?;
        Ok(count == 1)
    }
    async fn accept_system_query(
        &self,
        id: RequestId,
        query: &SystemQuery,
        device: DeviceRef,
        code: Option<DeviceCode>,
        actor: &str,
        owner: &str,
    ) -> Result<bool, RuntimeStoreError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing=sqlx::query("SELECT o.device_ref_json,o.initiated_by,o.owner_session_id,q.query_json FROM runtime_operations o LEFT JOIN runtime_system_results q ON q.id=o.id WHERE o.id=?").bind(id.to_string()).fetch_optional(&mut *tx).await?;
        if let Some(row) = existing {
            if row.try_get::<Option<String>, _>("query_json")?.is_none() {
                return Err(RuntimeStoreError::RequestConflict);
            }
            if serde_json::from_str::<DeviceRef>(row.try_get("device_ref_json")?)? != device
                || row.try_get::<String, _>("initiated_by")? != actor
                || row
                    .try_get::<Option<String>, _>("owner_session_id")?
                    .as_deref()
                    != Some(owner)
                || serde_json::from_str::<SystemQuery>(row.try_get("query_json")?)?
                    != query.persistence_form()
            {
                return Err(RuntimeStoreError::RequestConflict);
            }
            return Ok(false);
        }
        let command: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM runtime_tasks WHERE request_id=?")
                .bind(id.to_string())
                .fetch_one(&mut *tx)
                .await?;
        if command > 0 {
            return Err(RuntimeStoreError::RequestConflict);
        }
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM runtime_operations o JOIN runtime_system_results q ON q.id=o.id WHERE o.owner_session_id=? AND o.finished_at_unix_ms IS NULL").bind(owner).fetch_one(&mut *tx).await?;
        if count >= 16 {
            return Err(RuntimeStoreError::SystemQueryBusy);
        }
        sqlx::query("INSERT INTO runtime_operations (id,device_ref_json,device_code,initiated_by,kind,direction,source,destination,overwrite,state,started_at_unix_ms,owner_session_id) VALUES (?,?,?,?,?,?,?,'',0,'running',?,?)").bind(id.to_string()).bind(serde_json::to_string(&device)?).bind(code.map(|c|c.to_string())).bind(actor).bind(query.kind()).bind(if query.is_mutation() {"control"} else {"read"}).bind(match query {SystemQuery::Git{query}=>query.repo.as_str(),SystemQuery::Container{query}=>query.selector(),_=>""}).bind(now_unix_ms()).bind(owner).execute(&mut *tx).await?;
        let mut pending = SystemQueryReply::pending(id, query);
        pending.state = "unconfirmed".into();
        sqlx::query("INSERT INTO runtime_system_results (id,query_json,reply_json) VALUES (?,?,?)")
            .bind(id.to_string())
            .bind(serde_json::to_string(&query.persistence_form())?)
            .bind(serde_json::to_string(&pending)?)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }
    async fn system_record(
        &self,
        id: RequestId,
        code: Option<DeviceCode>,
        actor: &str,
        owner: &str,
    ) -> Result<(DeviceRef, SystemQueryReply), RuntimeStoreError> {
        let row=sqlx::query("SELECT o.device_ref_json,o.device_code,o.owner_session_id,s.stopped_at_unix_ms,s.heartbeat_at_unix_ms,q.reply_json FROM runtime_operations o JOIN runtime_system_results q ON q.id=o.id LEFT JOIN runtime_sessions s ON s.id=o.owner_session_id WHERE o.id=? AND o.initiated_by=?").bind(id.to_string()).bind(actor).fetch_optional(&self.pool).await?.ok_or(RuntimeStoreError::NotFound)?;
        if code.is_some_and(|c| {
            row.try_get::<Option<String>, _>("device_code")
                .ok()
                .flatten()
                .as_deref()
                != Some(c.to_string().as_str())
        }) {
            return Err(RuntimeStoreError::NotFound);
        }
        if row
            .try_get::<Option<String>, _>("owner_session_id")?
            .as_deref()
            != Some(owner)
            && row
                .try_get::<Option<i64>, _>("stopped_at_unix_ms")?
                .is_none()
            && row
                .try_get::<Option<i64>, _>("heartbeat_at_unix_ms")?
                .is_some_and(|t| t >= now_unix_ms() - 15_000)
        {
            return Err(RuntimeStoreError::NotFound);
        }
        Ok((
            serde_json::from_str(row.try_get("device_ref_json")?)?,
            serde_json::from_str(row.try_get("reply_json")?)?,
        ))
    }
    async fn save_system_reply(&self, r: &SystemQueryReply) -> Result<(), RuntimeStoreError> {
        let mut tx = self.pool.begin().await?;
        let terminal = matches!(
            r.state.as_str(),
            "completed" | "failed" | "interrupted" | "cancelled"
        );
        let changed=sqlx::query("UPDATE runtime_operations SET state=?,finished_at_unix_ms=?,size=?,offset=?,message=? WHERE id=? AND finished_at_unix_ms IS NULL").bind(if terminal {&r.state}else {"running"}).bind(terminal.then(now_unix_ms)).bind(r.returned_count as i64).bind(r.returned_count as i64).bind(&r.error).bind(r.request_id.to_string()).execute(&mut *tx).await?;
        if changed.rows_affected() == 1 {
            sqlx::query("UPDATE runtime_system_results SET reply_json=? WHERE id=?")
                .bind(serde_json::to_string(r)?)
                .bind(r.request_id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
impl BridgeRuntime {
    pub async fn cancel_system_query(
        &self,
        device: DeviceRef,
        id: RequestId,
    ) -> Result<SystemQueryReply, RuntimeError> {
        if !self
            .inner
            .store
            .owns_system_query(device, id, &self.inner.initiated_by, &self.inner.session_id)
            .await?
        {
            return Err(RuntimeStoreError::NotFound.into());
        }
        let (_, cached) = self
            .inner
            .store
            .system_record(id, None, &self.inner.initiated_by, &self.inner.session_id)
            .await?;
        if !cached.kind.starts_with("git_")
            && ![
                "containers",
                "container",
                "container_logs",
                "container_control",
            ]
            .contains(&cached.kind.as_str())
        {
            return Err(crate::BridgeError::UnexpectedTaskResponse(
                "only Docker/Git system operations can be cancelled".into(),
            )
            .into());
        }
        if matches!(
            cached.state.as_str(),
            "completed" | "failed" | "cancelled" | "interrupted"
        ) {
            return Ok(cached);
        }
        let reply = self
            .inner
            .device(device)
            .await
            .connection()
            .await?
            .cancel_system_query(id)
            .await?;
        if reply.kind != cached.kind {
            return Err(RuntimeStoreError::SnapshotIdentityMismatch.into());
        }
        self.inner.store.save_system_reply(&reply).await?;
        Ok(reply)
    }
    pub async fn system_query(
        &self,
        device: DeviceRef,
        id: RequestId,
        query: SystemQuery,
    ) -> Result<SystemQueryReply, RuntimeError> {
        query
            .validate()
            .map_err(|e| crate::BridgeError::UnexpectedTaskResponse(e.into()))?;
        let code = self.inner.device_codes.lock().await.get(&device).copied();
        if !self
            .inner
            .store
            .accept_system_query(
                id,
                &query,
                device,
                code,
                &self.inner.initiated_by,
                &self.inner.session_id,
            )
            .await?
        {
            return self.get_system_query(device, id).await;
        }
        let result = async {
            self.inner
                .device(device)
                .await
                .connection()
                .await?
                .system_query(id, query.clone())
                .await
                .map_err(RuntimeError::from)
        }
        .await;
        let reply = match result {
            Ok(r) => r,
            Err(e) => {
                let mut r = SystemQueryReply::pending(id, &query);
                r.state = if matches!(
                    e,
                    RuntimeError::Bridge(crate::BridgeError::UnsupportedSystemQuery)
                ) {
                    "failed"
                } else {
                    "unconfirmed"
                }
                .into();
                r.error = Some(e.to_string().chars().take(1024).collect());
                r
            }
        };
        self.inner.store.save_system_reply(&reply).await?;
        Ok(reply)
    }
    pub async fn get_system_query(
        &self,
        device: DeviceRef,
        id: RequestId,
    ) -> Result<SystemQueryReply, RuntimeError> {
        let (stored, cached) = self
            .inner
            .store
            .system_record(id, None, &self.inner.initiated_by, &self.inner.session_id)
            .await?;
        if stored != device {
            return Err(RuntimeStoreError::NotFound.into());
        }
        if matches!(
            cached.state.as_str(),
            "completed" | "failed" | "interrupted" | "cancelled"
        ) {
            return Ok(cached);
        }
        let lookup = async {
            self.inner
                .device(device)
                .await
                .connection()
                .await?
                .get_system_query(id)
                .await
                .map_err(RuntimeError::from)
        };
        let Ok(Ok(reply)) = tokio::time::timeout(std::time::Duration::from_secs(5), lookup).await
        else {
            return Ok(cached);
        };
        if reply.kind != cached.kind {
            return Err(RuntimeStoreError::SnapshotIdentityMismatch.into());
        }
        self.inner.store.save_system_reply(&reply).await?;
        Ok(self
            .inner
            .store
            .system_record(id, None, &self.inner.initiated_by, &self.inner.session_id)
            .await?
            .1)
    }
}
impl TransferQueue {
    pub async fn system_record(
        &self,
        id: RequestId,
        code: DeviceCode,
    ) -> Result<(DeviceRef, SystemQueryReply), RuntimeStoreError> {
        self.store
            .system_record(id, Some(code), &self.initiated_by, &self.session_id)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn container_audit_tracks_owner_target_and_unresolved_effects() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("bridge.sqlite3");
        let queue = TransferQueue::open(&database, "owner".into(), "guest".into())
            .await
            .unwrap();
        let store = RuntimeStore::open(&database).await.unwrap();
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        };
        let code: DeviceCode = "123456789".parse().unwrap();
        let id = RequestId::new();
        let q = SystemQuery::Container {
            query: ContainerQuery {
                action: ContainerAction::Control {
                    container: "api-service".into(),
                    control: ContainerControl::Restart,
                    stop_timeout_seconds: 10,
                },
                timeout_ms: 30000,
            },
        };
        assert!(
            store
                .accept_system_query(id, &q, device, Some(code), "guest", "owner")
                .await
                .unwrap()
        );
        assert!(
            !store
                .accept_system_query(id, &q, device, Some(code), "guest", "owner")
                .await
                .unwrap()
        );
        assert_eq!(queue.active_for_device(code).await.unwrap(), 1);
        assert!(
            !store
                .owns_system_query(device, id, "guest", "other")
                .await
                .unwrap()
        );
        let rows = store.operations().await.unwrap();
        assert_eq!(rows[0].kind, "container_control");
        assert_eq!(rows[0].source, "api-service");
        let mut r = SystemQueryReply::pending(id, &q);
        r.state = "unconfirmed".into();
        store.save_system_reply(&r).await.unwrap();
        assert_eq!(queue.active_for_device(code).await.unwrap(), 1);
        r.state = "completed".into();
        store.save_system_reply(&r).await.unwrap();
        assert_eq!(queue.active_for_device(code).await.unwrap(), 0);
    }
    #[tokio::test]
    async fn git_audit_protects_cancel_ownership_and_unresolved_work_blocks_disconnect() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("bridge.sqlite3");
        let queue = TransferQueue::open(&database, "owner".into(), "guest".into())
            .await
            .unwrap();
        let store = RuntimeStore::open(&database).await.unwrap();
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        };
        let code: DeviceCode = "123456789".parse().unwrap();
        let id = RequestId::new();
        let query = SystemQuery::Git {
            query: GitQuery {
                repo: "C:\\private-repo".into(),
                action: GitAction::Commit {
                    files: vec!["selected.txt".into()],
                    message: "private-message".into(),
                },
                timeout_ms: 30000,
            },
        };
        store
            .accept_system_query(id, &query, device, Some(code), "guest", "owner")
            .await
            .unwrap();
        assert_eq!(queue.active_for_device(code).await.unwrap(), 1);
        assert!(
            store
                .owns_system_query(device, id, "guest", "owner")
                .await
                .unwrap()
        );
        assert!(
            !store
                .owns_system_query(device, id, "guest", "other")
                .await
                .unwrap()
        );
        assert!(
            !store
                .owns_system_query(device, id, "foreign", "owner")
                .await
                .unwrap()
        );
        let rows = store.operations().await.unwrap();
        assert_eq!(rows[0].kind, "git_commit");
        assert_eq!(rows[0].source, "C:\\private-repo");
        let saved: String =
            sqlx::query_scalar("SELECT query_json FROM runtime_system_results WHERE id=?")
                .bind(id.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(!saved.contains("private-message"));
        let mut reply = SystemQueryReply::pending(id, &query);
        reply.state = "unconfirmed".into();
        store.save_system_reply(&reply).await.unwrap();
        assert_eq!(queue.active_for_device(code).await.unwrap(), 1);
        reply.state = "cancelled".into();
        store.save_system_reply(&reply).await.unwrap();
        assert_eq!(queue.active_for_device(code).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn desktop_audit_counts_active_operations_and_never_stores_raw_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.sqlite3");
        let queue = TransferQueue::open(&path, "owner".into(), "guest".into())
            .await
            .unwrap();
        let store = RuntimeStore::open(&path).await.unwrap();
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        };
        let code: DeviceCode = "123456789".parse().unwrap();
        let id = RequestId::new();
        let q = SystemQuery::Desktop {
            query: DesktopQuery::TypeText {
                window_ref: RequestId::new().to_string(),
                text: "private中文🙂".into(),
            },
        };
        assert!(
            store
                .accept_system_query(id, &q, device, Some(code), "guest", "owner")
                .await
                .unwrap()
        );
        assert_eq!(queue.active_for_device(code).await.unwrap(), 1);
        let saved: String =
            sqlx::query_scalar("SELECT query_json FROM runtime_system_results WHERE id=?")
                .bind(id.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(!saved.contains("private"));
        assert!(saved.contains("blake3:"));
        assert!(
            !store
                .accept_system_query(id, &q, device, Some(code), "guest", "owner")
                .await
                .unwrap()
        );
        assert!(
            store
                .accept_system_query(id, &q, device, Some(code), "other", "owner")
                .await
                .is_err()
        );
        let mut r = SystemQueryReply::pending(id, &q);
        r.state = "completed".into();
        r.data = Some(SystemQueryData::Desktop {
            snapshot: DesktopSnapshot::new("fixture".into(), "test"),
        });
        store.save_system_reply(&r).await.unwrap();
        assert_eq!(queue.active_for_device(code).await.unwrap(), 0);
        assert_eq!(queue.system_record(id, code).await.unwrap().1, r);
        assert_eq!(
            queue
                .operation_page(Some(code), None, None, 20)
                .await
                .unwrap()[0]["kind"],
            "type_text"
        );
    }
    #[tokio::test]
    async fn desktop_mutations_count_as_active_and_persist_failure_without_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.sqlite3");
        let queue = TransferQueue::open(&path, "owner".into(), "guest".into())
            .await
            .unwrap();
        let store = RuntimeStore::open(&path).await.unwrap();
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        };
        let code = "123456789".parse().unwrap();
        let batch = SystemQuery::Desktop {
            query: DesktopQuery::Batch {
                window_ref: RequestId::new().to_string(),
                actions: vec![DesktopAction::TypeText {
                    text: "private batch".into(),
                }],
                timeout_ms: 5000,
            },
        };
        let monitor: SystemQuery = serde_json::from_value(serde_json::json!({"action":"desktop","query":{
            "operation":"monitor_input","input":{"target":{"helper_instance":RequestId::new(),
            "id":1,"x":0,"y":0,"width":1920,"height":1080,"scale_percent":100,"rotation_degrees":0,
            "coordinate_space":"physical_pixels"},"action":{"type":"move","x":10,"y":20}}
        }})).unwrap();
        for q in [batch, monitor] {
            let id = RequestId::new();
            assert!(
                store
                    .accept_system_query(id, &q, device, Some(code), "guest", "owner")
                    .await
                    .unwrap()
            );
            assert_eq!(queue.active_for_device(code).await.unwrap(), 1);
            let saved: String =
                sqlx::query_scalar("SELECT query_json FROM runtime_system_results WHERE id=?")
                    .bind(id.to_string())
                    .fetch_one(&store.pool)
                    .await
                    .unwrap();
            assert!(!saved.contains("private batch"));
            let mut reply = SystemQueryReply::pending(id, &q);
            reply.state = "failed".into();
            reply.error = Some("batch stopped".into());
            store.save_system_reply(&reply).await.unwrap();
            assert_eq!(queue.active_for_device(code).await.unwrap(), 0);
            assert!(
                !store
                    .accept_system_query(id, &q, device, Some(code), "guest", "owner")
                    .await
                    .unwrap()
            );
            assert_eq!(queue.system_record(id, code).await.unwrap().1, reply);
        }
    }
    #[tokio::test]
    async fn lifecycle_records_are_control_operations_and_preserve_partial_failures_without_replay()
    {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.sqlite3");
        let queue = TransferQueue::open(&path, "owner".into(), "guest".into())
            .await
            .unwrap();
        let store = RuntimeStore::open(&path).await.unwrap();
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        };
        let code = "123456789".parse().unwrap();
        let id = RequestId::new();
        let q = SystemQuery::ServiceControl {
            name: "fixture".into(),
            control: ServiceControlAction::Restart,
            timeout_ms: 100,
        };
        assert!(
            store
                .accept_system_query(id, &q, device, Some(code), "guest", "owner")
                .await
                .unwrap()
        );
        assert_eq!(store.operations().await.unwrap()[0].direction, "control");
        assert_eq!(queue.active_for_device(code).await.unwrap(), 1);
        let mut r = SystemQueryReply::pending(id, &q);
        r.state = "failed".into();
        r.error = Some("timeout after service stopped".into());
        r.data = Some(SystemQueryData::ServiceControl {
            result: ServiceControlResult {
                name: "fixture".into(),
                control: ServiceControlAction::Restart,
                outcome: "timeout".into(),
                phase: "starting".into(),
                changed: true,
                job_path: None,
                service: None,
                error: r.error.clone(),
            },
        });
        store.save_system_reply(&r).await.unwrap();
        assert_eq!(queue.active_for_device(code).await.unwrap(), 0);
        assert!(
            !store
                .accept_system_query(id, &q, device, Some(code), "guest", "owner")
                .await
                .unwrap()
        );
        assert_eq!(queue.system_record(id, code).await.unwrap().1, r);
        let wrong = SystemQuery::ServiceControl {
            name: "other".into(),
            control: ServiceControlAction::Restart,
            timeout_ms: 100,
        };
        assert!(
            store
                .accept_system_query(id, &wrong, device, Some(code), "guest", "owner")
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn sampled_results_are_owned_cached_listed_and_terminal_results_never_regress() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.sqlite3");
        let queue = TransferQueue::open(&path, "owner".into(), "guest".into())
            .await
            .unwrap();
        let store = RuntimeStore::open(&path).await.unwrap();
        store.start_session("other").await.unwrap();
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        };
        let code = "123456789".parse().unwrap();
        let id = RequestId::new();
        let query = SystemQuery::Disks { limit: 100 };
        assert!(
            store
                .accept_system_query(id, &query, device, Some(code), "guest", "owner")
                .await
                .unwrap()
        );
        assert!(
            !store
                .accept_system_query(id, &query, device, Some(code), "guest", "owner")
                .await
                .unwrap()
        );
        assert_eq!(queue.active_for_device(code).await.unwrap(), 1);
        let operation = store.operations().await.unwrap().remove(0);
        assert_eq!(
            operation.execution_observation.as_deref(),
            Some("unconfirmed")
        );
        assert!(
            store
                .system_record(id, Some(code), "guest", "other")
                .await
                .is_err()
        );
        assert!(
            store
                .system_record(id, Some("987654321".parse().unwrap()), "guest", "owner")
                .await
                .is_err()
        );
        assert!(
            store
                .system_record(id, Some(code), "other-actor", "owner")
                .await
                .is_err()
        );
        let mut r = SystemQueryReply::pending(id, &query);
        r.state = "completed".into();
        r.sampled_at_unix_ms = Some(10);
        r.data = Some(SystemQueryData::Disks { entries: vec![] });
        store.save_system_reply(&r).await.unwrap();
        let page = queue
            .operation_page(Some(code), None, None, 20)
            .await
            .unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0]["kind"], "disks");
        assert_eq!(page[0]["updated_at_unix_ms"], 10);
        r.state = "unconfirmed".into();
        store.save_system_reply(&r).await.unwrap();
        assert_eq!(
            queue.system_record(id, code).await.unwrap().1.state,
            "completed"
        );
        assert_eq!(queue.active_for_device(code).await.unwrap(), 0);
        store.stop_session("owner").await.unwrap();
        assert_eq!(
            store
                .system_record(id, Some(code), "guest", "other")
                .await
                .unwrap()
                .1
                .state,
            "completed"
        );
    }
    #[tokio::test]
    async fn unresolved_query_cap_rejects_new_requests_without_preventing_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let store = RuntimeStore::open(&dir.path().join("bridge.sqlite3"))
            .await
            .unwrap();
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        };
        let q = SystemQuery::Disks { limit: 1 };
        let mut last = RequestId::new();
        for _ in 0..16 {
            last = RequestId::new();
            assert!(
                store
                    .accept_system_query(last, &q, device, None, "guest", "owner")
                    .await
                    .unwrap()
            );
        }
        assert!(
            !store
                .accept_system_query(last, &q, device, None, "guest", "owner")
                .await
                .unwrap()
        );
        assert!(matches!(
            store
                .accept_system_query(RequestId::new(), &q, device, None, "guest", "owner")
                .await,
            Err(RuntimeStoreError::SystemQueryBusy)
        ));
    }
}
