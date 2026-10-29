//! Connection-bound PTYs. Accepted input sequences are journaled before dispatch.
use super::*;
use pab_terminal::TerminalSession;
use std::sync::{
    Weak,
    atomic::{AtomicBool, Ordering},
};

pub(crate) struct ConnectionScope {
    pub id: String,
    pub(super) closed: AtomicBool,
}
pub(crate) struct ConnectionLease {
    pub scope: Arc<ConnectionScope>,
    service: Weak<ExecutionService>,
}
impl Drop for ConnectionLease {
    fn drop(&mut self) {
        self.scope.closed.store(true, Ordering::Release);
        if let (Some(service), Ok(runtime)) = (
            self.service.upgrade(),
            tokio::runtime::Handle::try_current(),
        ) {
            let id = self.scope.id.clone();
            runtime.spawn(async move {
                service.close_terminals(Some(&id)).await;
            });
        }
    }
}
pub(super) struct Entry {
    owner: String,
    scope: Arc<ConnectionScope>,
    session: Arc<TerminalSession>,
    action: Mutex<u64>,
    closed: AtomicBool,
    last_used: Mutex<std::time::Instant>,
    cols: u16,
    rows: u16,
    uid: u32,
    root: PathBuf,
}

impl ExecutionService {
    pub fn connection_lease(self: &Arc<Self>) -> ConnectionLease {
        ConnectionLease {
            scope: Arc::new(ConnectionScope {
                id: uuid::Uuid::new_v4().to_string(),
                closed: AtomicBool::new(false),
            }),
            service: Arc::downgrade(self),
        }
    }

    async fn terminal_open(
        &self,
        owner: &str,
        scope: &Arc<ConnectionScope>,
        id: RequestId,
        cols: u16,
        rows: u16,
        selection: ExecutionSelection,
    ) -> Result<DeviceTaskResponse> {
        if !(20..=500).contains(&cols) || !(5..=200).contains(&rows) {
            return Err(invalid("invalid_terminal_size"));
        }
        let digest = self.digest(
            &serde_json::to_vec(&(cols, rows, selection))
                .map_err(|_| invalid("invalid_terminal"))?,
        );
        let _accept = self.acceptance.lock().await;
        if let Some(row) = sqlx::query("SELECT owner,digest FROM terminals WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
        {
            if row.get::<String, _>("owner") != owner {
                return Err(missing());
            }
            if row.get::<String, _>("digest") != digest {
                return Err(ExecError(
                    DeviceTaskErrorCode::RequestConflict,
                    "request_conflict",
                ));
            }
            let entry = self.terminal_entry(owner, scope, id).await?;
            if entry.closed.load(Ordering::Acquire) {
                return Err(invalid("terminal_ended_original_not_reopened"));
            }
            return Ok(opened(id, &entry));
        }
        if !selection.is_service() {
            return Err(ExecError(
                DeviceTaskErrorCode::Unsupported,
                "android_terminal_requires_service_execution",
            ));
        }
        let (root, uid) = self.authorize().await?;
        if scope.closed.load(Ordering::Acquire) || self.stopping.load(Ordering::Acquire) {
            return Err(denied("terminal_connection_closed"));
        }
        if self.terminals.lock().await.len() >= 2 {
            return Err(denied("terminal_capacity"));
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM terminals")
            .fetch_one(&self.pool)
            .await?;
        if count >= 10000 {
            return Err(internal("terminal_journal_capacity"));
        }
        sqlx::query("INSERT INTO terminals(owner,id,digest,state) VALUES(?,?,?,'accepted')")
            .bind(owner)
            .bind(id.to_string())
            .bind(digest)
            .execute(&self.pool)
            .await?;
        let cwd = root.to_string_lossy().into_owned();
        let env = environment(&root);
        // Cancellation of a transport stream must not drop a half-open PTY. This
        // short bounded spawn runs synchronously under the acceptance lock.
        let session =
            match TerminalSession::start_in("/system/bin/sh", &["-i"], cols, rows, &cwd, &env) {
                Ok(session) => Arc::new(session),
                Err(_) => {
                    sqlx::query("UPDATE terminals SET state='failed' WHERE id=?")
                        .bind(id.to_string())
                        .execute(&self.pool)
                        .await?;
                    return Err(ExecError(
                        DeviceTaskErrorCode::Unsupported,
                        "android_pty_start_failed",
                    ));
                }
            };
        let entry = Arc::new(Entry {
            owner: owner.into(),
            scope: scope.clone(),
            session,
            action: Mutex::new(1),
            closed: AtomicBool::new(false),
            last_used: Mutex::new(std::time::Instant::now()),
            cols,
            rows,
            uid,
            root,
        });
        self.terminals.lock().await.insert(id, entry.clone());
        if scope.closed.load(Ordering::Acquire) || self.stopping.load(Ordering::Acquire) {
            let _ = entry.session.close();
            entry.closed.store(true, Ordering::Release);
            self.terminals.lock().await.remove(&id);
            return Err(denied("terminal_connection_closed"));
        }
        if sqlx::query("UPDATE terminals SET state='running' WHERE id=?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .is_err()
        {
            let _ = entry.session.close();
            entry.closed.store(true, Ordering::Release);
            self.terminals.lock().await.remove(&id);
            return Err(internal("terminal_open_outcome_unconfirmed"));
        }
        Ok(opened(id, &entry))
    }

    async fn terminal_entry(
        &self,
        owner: &str,
        scope: &Arc<ConnectionScope>,
        id: RequestId,
    ) -> Result<Arc<Entry>> {
        let entry = self
            .terminals
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or_else(missing)?;
        if entry.owner != owner
            || entry.scope.id != scope.id
            || scope.closed.load(Ordering::Acquire)
        {
            return Err(missing());
        }
        *entry.last_used.lock().await = std::time::Instant::now();
        Ok(entry)
    }

    pub(super) async fn close_terminals(&self, scope: Option<&str>) {
        self.close_desktop_terminals(scope).await;
        let entries: Vec<_> = self
            .terminals
            .lock()
            .await
            .iter()
            .filter(|(_, entry)| scope.is_none_or(|id| entry.scope.id == id))
            .map(|(id, entry)| (*id, entry.clone()))
            .collect();
        for (id, entry) in entries {
            let _action = entry.action.lock().await;
            if !entry.closed.swap(true, Ordering::AcqRel) {
                let _ = entry.session.close();
            }
            let _ = sqlx::query("UPDATE terminals SET state='interrupted' WHERE id=?")
                .bind(id.to_string())
                .execute(&self.pool)
                .await;
            self.terminals.lock().await.remove(&id);
        }
    }

    pub(super) async fn expire_terminals(&self) {
        let entries: Vec<_> = self
            .terminals
            .lock()
            .await
            .iter()
            .map(|(id, entry)| (*id, entry.clone()))
            .collect();
        for (id, entry) in entries {
            if entry.last_used.lock().await.elapsed() < Duration::from_secs(900) {
                continue;
            }
            let _action = entry.action.lock().await;
            if !entry.closed.swap(true, Ordering::AcqRel) {
                let _ = entry.session.close();
            }
            let _ = sqlx::query("UPDATE terminals SET state='expired' WHERE id=?")
                .bind(id.to_string())
                .execute(&self.pool)
                .await;
            self.terminals.lock().await.remove(&id);
        }
    }

    async fn terminal_action(
        &self,
        entry: &Entry,
        id: RequestId,
        seq: u64,
        kind: &str,
        payload: &[u8],
        size: Option<(u16, u16)>,
    ) -> Result<DeviceTaskResponse> {
        let mut next = entry.action.lock().await;
        let digest = self.digest(&[kind.as_bytes(), payload].concat());
        if let Some(row) =
            sqlx::query("SELECT digest,state FROM terminal_actions WHERE id=? AND seq=?")
                .bind(id.to_string())
                .bind(seq.min(i64::MAX as u64) as i64)
                .fetch_optional(&self.pool)
                .await?
        {
            if row.get::<String, _>("digest") != digest {
                return Err(ExecError(
                    DeviceTaskErrorCode::RequestConflict,
                    "terminal_sequence_conflict",
                ));
            }
            if row.get::<String, _>("state") != "completed" {
                let _ = entry.session.close();
                entry.closed.store(true, Ordering::Release);
                return Err(internal("terminal_action_unconfirmed_not_replayed"));
            }
            return Ok(acknowledged(id, seq, kind));
        }
        if entry.closed.load(Ordering::Acquire) {
            return Err(invalid("terminal_closed"));
        }
        if seq != *next || seq > 10000 {
            return Err(invalid("terminal_sequence_mismatch_or_capacity"));
        }
        if kind != "close" {
            self.authorize().await?;
            if entry.scope.closed.load(Ordering::Acquire) || self.stopping.load(Ordering::Acquire) {
                return Err(denied("terminal_connection_closed"));
            }
        }
        sqlx::query("INSERT INTO terminal_actions(id,seq,digest,state) VALUES(?,?,?,'accepted')")
            .bind(id.to_string())
            .bind(seq as i64)
            .bind(digest)
            .execute(&self.pool)
            .await?;
        *next += 1;
        let result = match kind {
            "input" => entry.session.input(payload),
            "resize" => {
                let (cols, rows) = size.expect("validated resize");
                entry.session.resize(cols, rows)
            }
            _ => entry.session.close(),
        };
        let state = if result.is_ok() {
            "completed"
        } else {
            "unconfirmed"
        };
        let saved = sqlx::query("UPDATE terminal_actions SET state=? WHERE id=? AND seq=?")
            .bind(state)
            .bind(id.to_string())
            .bind(seq as i64)
            .execute(&self.pool)
            .await;
        if result.is_err() || saved.is_err() {
            let _ = entry.session.close();
            entry.closed.store(true, Ordering::Release);
            let _ = sqlx::query("UPDATE terminals SET state='interrupted' WHERE id=?")
                .bind(id.to_string())
                .execute(&self.pool)
                .await;
            return Err(internal("terminal_action_unconfirmed_not_replayed"));
        }
        if kind == "close" {
            entry.closed.store(true, Ordering::Release);
            sqlx::query("UPDATE terminals SET state='closed' WHERE id=?")
                .bind(id.to_string())
                .execute(&self.pool)
                .await?;
        }
        Ok(acknowledged(id, seq, kind))
    }

    async fn dispatch_terminal(
        self: &Arc<Self>,
        entry: Arc<Entry>,
        id: RequestId,
        seq: u64,
        kind: &'static str,
        payload: Vec<u8>,
        size: Option<(u16, u16)>,
    ) -> Result<DeviceTaskResponse> {
        let service = self.clone();
        self.detached(async move {
            service
                .terminal_action(&entry, id, seq, kind, &payload, size)
                .await
        })
        .await
    }

    pub(super) async fn serve_terminal(
        self: &Arc<Self>,
        owner: &str,
        scope: &Arc<ConnectionScope>,
        request: &DeviceTaskRequest,
        stream: &mut PabBiStream,
    ) -> std::result::Result<bool, PabConnectionError> {
        let timeout = Duration::from_secs(8);
        let result = match request {
            DeviceTaskRequest::OpenTerminal {
                request_id,
                cols,
                rows,
                execution,
                ..
            } => {
                let service = self.clone();
                let owner = owner.to_owned();
                let scope = scope.clone();
                let id = *request_id;
                let cols = *cols;
                let rows = *rows;
                let execution = *execution;
                self.detached(async move {
                    service
                        .terminal_open(&owner, &scope, id, cols, rows, execution)
                        .await
                })
                .await
            }
            DeviceTaskRequest::TerminalInput {
                session_id,
                sequence,
                size,
                ..
            } => {
                if *size == 0 || *size as usize > MAX_TERMINAL_INPUT_BYTES {
                    Err(invalid("invalid_terminal_input_size"))
                } else {
                    let bytes = stream.receive_binary_frame(timeout).await?;
                    stream.expect_receive_end(timeout).await?;
                    if bytes.len() != *size as usize {
                        Err(invalid("terminal_input_size_mismatch"))
                    } else {
                        match self.terminal_entry(owner, scope, *session_id).await {
                            Ok(entry) => {
                                self.dispatch_terminal(
                                    entry,
                                    *session_id,
                                    *sequence,
                                    "input",
                                    bytes,
                                    None,
                                )
                                .await
                            }
                            Err(error) => Err(error),
                        }
                    }
                }
            }
            DeviceTaskRequest::TerminalResize {
                session_id,
                sequence,
                cols,
                rows,
                ..
            } => {
                if !(20..=500).contains(cols) || !(5..=200).contains(rows) {
                    Err(invalid("invalid_terminal_size"))
                } else {
                    match self.terminal_entry(owner, scope, *session_id).await {
                        Ok(entry) => {
                            self.dispatch_terminal(
                                entry,
                                *session_id,
                                *sequence,
                                "resize",
                                format!("{cols}x{rows}").into_bytes(),
                                Some((*cols, *rows)),
                            )
                            .await
                        }
                        Err(error) => Err(error),
                    }
                }
            }
            DeviceTaskRequest::TerminalClose {
                session_id,
                sequence,
                ..
            } => match self.terminal_entry(owner, scope, *session_id).await {
                Ok(entry) => {
                    self.dispatch_terminal(entry, *session_id, *sequence, "close", Vec::new(), None)
                        .await
                }
                Err(error) => Err(error),
            },
            DeviceTaskRequest::TerminalRead {
                session_id,
                offset,
                limit,
                ..
            } => {
                if *limit == 0 || *limit as usize > MAX_TERMINAL_READ_BYTES {
                    Err(invalid("invalid_terminal_read_size"))
                } else {
                    match self.terminal_entry(owner, scope, *session_id).await {
                        Err(error) => Err(error),
                        Ok(entry) => {
                            if let Err(error) = self.authorize().await {
                                stream.send_json(&error.wire(), timeout).await?;
                                return Ok(true);
                            }
                            let _action = entry.action.lock().await;
                            let output = entry.session.read(*offset, *limit as usize);
                            let response = DeviceTaskResponse::TerminalOutput {
                                session_id: *session_id,
                                retained_from: output.retained_from,
                                offset: output.offset,
                                next_offset: output.next_offset,
                                size: output.bytes.len() as u16,
                                ended: output.ended,
                            };
                            stream.send_frame_json(&response, timeout).await?;
                            if !output.bytes.is_empty() {
                                stream.send_binary_frame(&output.bytes, timeout).await?;
                            }
                            stream.finish_send(timeout).await?;
                            if output.ended {
                                let _ = entry.session.close();
                                entry.closed.store(true, Ordering::Release);
                                let _ =
                                    sqlx::query("UPDATE terminals SET state='closed' WHERE id=?")
                                        .bind(session_id.to_string())
                                        .execute(&self.pool)
                                        .await;
                                self.terminals.lock().await.remove(session_id);
                            }
                            return Ok(true);
                        }
                    }
                }
            }
            _ => return Ok(false),
        };
        stream
            .send_json(&result.unwrap_or_else(ExecError::wire), timeout)
            .await?;
        Ok(true)
    }
}

fn opened(id: RequestId, entry: &Entry) -> DeviceTaskResponse {
    DeviceTaskResponse::TerminalOpened {
        session_id: id,
        shell: "/system/bin/sh".into(),
        startup: Some(TerminalStartup {
            arguments: vec!["-i".into()],
            mode: TerminalStartupMode::Interactive,
        }),
        cols: entry.cols,
        rows: entry.rows,
        identity: Some(identity(entry.uid, &entry.root)),
    }
}

fn acknowledged(id: RequestId, seq: u64, kind: &str) -> DeviceTaskResponse {
    if kind == "close" {
        DeviceTaskResponse::TerminalClosed { session_id: id }
    } else {
        DeviceTaskResponse::TerminalAcknowledged {
            session_id: id,
            sequence: seq,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture(
        root: &Path,
    ) -> (
        Arc<ExecutionService>,
        ConnectionLease,
        RequestId,
        Arc<Entry>,
        tokio::task::JoinHandle<()>,
    ) {
        let device = DeviceRef {
            tenant_id: TenantId::new(),
            device_id: DeviceId::new(),
        };
        let service = ExecutionService::open(&root.join("terminal.sqlite"), [5; 32], device)
            .await
            .unwrap();
        let lease = service.connection_lease();
        let id = RequestId::new();
        #[cfg(windows)]
        let (shell, args): (&str, &[&str]) = ("cmd.exe", &["/D", "/K"]);
        #[cfg(not(windows))]
        let (shell, args): (&str, &[&str]) = ("/bin/sh", &["-i"]);
        let env = std::env::vars().collect();
        let session =
            TerminalSession::start_in(shell, args, 80, 24, &root.to_string_lossy(), &env).unwrap();
        let entry = Arc::new(Entry {
            owner: "owner".into(),
            scope: lease.scope.clone(),
            session: Arc::new(session),
            action: Mutex::new(1),
            closed: AtomicBool::new(false),
            last_used: Mutex::new(std::time::Instant::now()),
            cols: 80,
            rows: 24,
            uid: 12345,
            root: root.to_owned(),
        });
        service.terminals.lock().await.insert(id, entry.clone());
        sqlx::query(
            "INSERT INTO terminals(owner,id,digest,state) VALUES('owner',?,'test','running')",
        )
        .bind(id.to_string())
        .execute(&service.pool)
        .await
        .unwrap();
        let worker = service.clone();
        let root = root.to_owned();
        let gate = tokio::spawn(async move {
            loop {
                let job = worker.sdk.poll().await;
                if job.is_null() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    continue;
                }
                worker
                    .sdk
                    .complete(
                        serde_json::from_value(job["ticket"].clone()).unwrap(),
                        json!({"workspace":root,"uid":12345}),
                    )
                    .await;
            }
        });
        (service, lease, id, entry, gate)
    }

    fn input() -> Vec<u8> {
        #[cfg(windows)]
        {
            b"echo x>> once.txt\r\n".to_vec()
        }
        #[cfg(not(windows))]
        {
            b"printf 'x\\n' >> once.txt\n".to_vec()
        }
    }
    async fn expect_once(root: &Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(value) = std::fs::read_to_string(root.join("once.txt"))
                && !value.is_empty()
            {
                assert_eq!(value.lines().collect::<Vec<_>>(), vec!["x"]);
                return;
            }
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn real_terminal_input_is_owner_connection_and_sequence_bound() {
        let temp = tempfile::tempdir().unwrap();
        let (service, lease, id, entry, gate) = fixture(temp.path()).await;
        let other = service.connection_lease();
        assert!(
            service
                .terminal_entry("other", &lease.scope, id)
                .await
                .is_err()
        );
        assert!(
            service
                .terminal_entry("owner", &other.scope, id)
                .await
                .is_err()
        );
        service
            .dispatch_terminal(entry.clone(), id, 1, "input", input(), None)
            .await
            .unwrap();
        expect_once(temp.path()).await;
        service
            .dispatch_terminal(entry.clone(), id, 1, "input", input(), None)
            .await
            .unwrap();
        assert!(matches!(
            service
                .dispatch_terminal(entry.clone(), id, 1, "input", b"different".to_vec(), None)
                .await,
            Err(ExecError(DeviceTaskErrorCode::RequestConflict, _))
        ));
        service
            .dispatch_terminal(
                entry.clone(),
                id,
                2,
                "resize",
                b"100x35".to_vec(),
                Some((100, 35)),
            )
            .await
            .unwrap();
        let reply = service
            .dispatch_terminal(entry, id, 3, "close", Vec::new(), None)
            .await
            .unwrap();
        assert!(matches!(reply,DeviceTaskResponse::TerminalClosed{session_id} if session_id==id));
        expect_once(temp.path()).await;
        gate.abort();
        let _ = gate.await;
        service.shutdown().await;
    }

    #[tokio::test]
    async fn input_response_persistence_failure_closes_pty_and_never_replays() {
        let temp = tempfile::tempdir().unwrap();
        let (service, _lease, id, entry, gate) = fixture(temp.path()).await;
        sqlx::query("CREATE TRIGGER fail_completion BEFORE UPDATE ON terminal_actions BEGIN SELECT RAISE(FAIL,'owned fault'); END").execute(&service.pool).await.unwrap();
        let first = service
            .dispatch_terminal(entry.clone(), id, 1, "input", input(), None)
            .await;
        assert!(matches!(
            first,
            Err(ExecError(
                DeviceTaskErrorCode::Internal,
                "terminal_action_unconfirmed_not_replayed"
            ))
        ));
        assert!(entry.closed.load(Ordering::Acquire));
        let again = service
            .dispatch_terminal(entry, id, 1, "input", input(), None)
            .await;
        assert!(matches!(
            again,
            Err(ExecError(
                DeviceTaskErrorCode::Internal,
                "terminal_action_unconfirmed_not_replayed"
            ))
        ));
        let state: String =
            sqlx::query_scalar("SELECT state FROM terminal_actions WHERE id=? AND seq=1")
                .bind(id.to_string())
                .fetch_one(&service.pool)
                .await
                .unwrap();
        assert_eq!(state, "accepted");
        if temp.path().join("once.txt").exists() {
            expect_once(temp.path()).await;
        }
        gate.abort();
        let _ = gate.await;
        service.shutdown().await;
    }
}
