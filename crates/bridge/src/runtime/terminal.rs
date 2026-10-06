use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use pab_protocol::{DeviceRef, MAX_TERMINAL_INPUT_BYTES, RequestId};
use sqlx::Row;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::connection::{BridgeError, TerminalOpened, TerminalOutput};

use super::store::{RuntimeStore, RuntimeStoreError};
use super::{BridgeRuntime, RuntimeError};

const MAX_TERMINAL_ARCHIVE_BYTES: u64 = 16 * 1024 * 1024;

pub(super) struct TerminalRuntimeSession {
    device_ref: DeviceRef,
    next_sequence: u64,
    offset: u64,
}

pub(super) fn terminal_dir(database_path: &Path) -> PathBuf {
    database_path.with_extension("terminals")
}

pub(super) async fn read_terminal(
    store: &RuntimeStore,
    directory: &Path,
    id: &str,
) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Ok(None);
    }
    let row =
        sqlx::query("SELECT offset FROM runtime_operations WHERE id = ? AND kind = 'terminal'")
            .bind(id)
            .fetch_optional(&store.pool)
            .await?;
    let Some(row) = row else { return Ok(None) };
    let recorded_size: i64 = row.try_get("offset")?;
    let path = directory.join(format!("{id}.bin"));
    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(RuntimeStoreError::Io(error)),
    };
    if metadata.len() > MAX_TERMINAL_ARCHIVE_BYTES
        || i64::try_from(metadata.len()).ok() != Some(recorded_size)
    {
        return Ok(None);
    }
    tokio::fs::read(path)
        .await
        .map(Some)
        .map_err(RuntimeStoreError::Io)
}

impl BridgeRuntime {
    pub async fn open_terminal(
        &self,
        device_ref: DeviceRef,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalOpened, RuntimeError> {
        self.open_terminal_as(device_ref, cols, rows, Default::default())
            .await
    }

    pub async fn open_terminal_as(
        &self,
        device_ref: DeviceRef,
        cols: u16,
        rows: u16,
        execution: pab_protocol::ExecutionSelection,
    ) -> Result<TerminalOpened, RuntimeError> {
        validate_terminal_size(cols, rows)?;
        // A read-only probe refreshes a cached device connection before a non-idempotent open.
        self.current_environment(device_ref).await?;
        let id = RequestId::new();
        let path = self.inner.terminal_dir.join(format!("{id}.bin"));
        if let Some(parent) = path.parent() {
            pab_agent_core::ensure_data_dir(parent)
                .map_err(|error| RuntimeError::Bridge(BridgeError::LocalFile(error)))?;
        }
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
            .map_err(|error| RuntimeError::Bridge(BridgeError::LocalFile(error)))?;
        drop(file);
        let code = self
            .inner
            .device_codes
            .lock()
            .await
            .get(&device_ref)
            .copied();
        if let Err(error) = self
            .inner
            .store
            .start_terminal_operation(
                id,
                device_ref,
                code,
                &self.inner.initiated_by,
                &path.to_string_lossy(),
                &self.inner.session_id,
            )
            .await
        {
            let _ = tokio::fs::remove_file(path).await;
            return Err(error.into());
        }
        let device = self.inner.device(device_ref).await;
        let result = match device.connection().await {
            Ok(connection) => match connection.open_terminal_as(id, cols, rows, execution).await {
                Ok(opened) => Ok(opened),
                Err(error) => {
                    if error.is_recoverable_connection() {
                        let _ = device.recover(&connection, &error).await;
                    }
                    Err(RuntimeError::Bridge(error))
                }
            },
            Err(error) => Err(error),
        };
        match result {
            Ok(opened) => {
                self.inner.terminals.lock().await.insert(
                    id,
                    Arc::new(Mutex::new(TerminalRuntimeSession {
                        device_ref,
                        next_sequence: 1,
                        offset: 0,
                    })),
                );
                Ok(opened)
            }
            Err(error) => {
                let state = if matches!(
                    &error,
                    RuntimeError::Bridge(source) if source.is_recoverable_connection()
                ) {
                    "unconfirmed"
                } else {
                    "failed"
                };
                self.inner
                    .store
                    .finish_operation(id, state, Some(&error.to_string()))
                    .await?;
                let _ = tokio::fs::remove_file(path).await;
                Err(error)
            }
        }
    }

    pub async fn terminal_input(&self, id: RequestId, bytes: &[u8]) -> Result<(), RuntimeError> {
        if bytes.is_empty() || bytes.len() > MAX_TERMINAL_INPUT_BYTES {
            return Err(RuntimeError::Bridge(BridgeError::Terminal(
                "terminal input must be between 1 and 4096 bytes".to_owned(),
            )));
        }
        let entry = self.terminal_entry(id).await?;
        let mut session = entry.lock().await;
        let sequence = session.next_sequence;
        self.inner
            .store
            .begin_terminal_event(id, sequence, "input", bytes)
            .await?;
        session.next_sequence += 1;
        let result: Result<_, RuntimeError> = async {
            let connection = self
                .inner
                .device(session.device_ref)
                .await
                .connection()
                .await?;
            Ok(connection.terminal_input(id, sequence, bytes).await?)
        }
        .await;
        self.inner
            .store
            .finish_terminal_event(
                id,
                sequence,
                if result.is_ok() {
                    "completed"
                } else {
                    "unconfirmed"
                },
            )
            .await?;
        result
    }

    pub async fn terminal_read(&self, id: RequestId) -> Result<TerminalOutput, RuntimeError> {
        let entry = self.terminal_entry(id).await?;
        let mut session = entry.lock().await;
        let connection = self
            .inner
            .device(session.device_ref)
            .await
            .connection()
            .await?;
        let output = connection.terminal_read(id, session.offset).await?;
        if output.offset != session.offset || output.next_offset > MAX_TERMINAL_ARCHIVE_BYTES {
            self.inner
                .store
                .finish_operation(
                    id,
                    "failed",
                    Some("terminal output exceeded the retained range or 16 MiB archive limit"),
                )
                .await?;
            return Err(RuntimeError::Bridge(BridgeError::Terminal(
                "terminal output cannot be archived completely".to_owned(),
            )));
        }
        if !output.bytes.is_empty() {
            let path = self.inner.terminal_dir.join(format!("{id}.bin"));
            let mut file = tokio::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .await
                .map_err(|error| RuntimeError::Bridge(BridgeError::LocalFile(error)))?;
            file.write_all(&output.bytes)
                .await
                .map_err(|error| RuntimeError::Bridge(BridgeError::LocalFile(error)))?;
            file.sync_all()
                .await
                .map_err(|error| RuntimeError::Bridge(BridgeError::LocalFile(error)))?;
            session.offset = output.next_offset;
            self.inner
                .store
                .operation_progress(id, session.offset, session.offset)
                .await?;
        }
        if output.ended {
            self.inner
                .store
                .finish_operation(id, "completed", None)
                .await?;
            drop(session);
            self.inner.terminals.lock().await.remove(&id);
        }
        Ok(output)
    }

    pub async fn terminal_resize(
        &self,
        id: RequestId,
        cols: u16,
        rows: u16,
    ) -> Result<(), RuntimeError> {
        validate_terminal_size(cols, rows)?;
        let entry = self.terminal_entry(id).await?;
        let mut session = entry.lock().await;
        let sequence = session.next_sequence;
        let payload = format!("{cols}x{rows}");
        self.inner
            .store
            .begin_terminal_event(id, sequence, "resize", payload.as_bytes())
            .await?;
        session.next_sequence += 1;
        let result: Result<_, RuntimeError> = async {
            let connection = self
                .inner
                .device(session.device_ref)
                .await
                .connection()
                .await?;
            Ok(connection.terminal_resize(id, sequence, cols, rows).await?)
        }
        .await;
        self.inner
            .store
            .finish_terminal_event(
                id,
                sequence,
                if result.is_ok() {
                    "completed"
                } else {
                    "unconfirmed"
                },
            )
            .await?;
        result
    }

    pub async fn terminal_close(&self, id: RequestId) -> Result<(), RuntimeError> {
        let entry = self.terminal_entry(id).await?;
        let mut session = entry.lock().await;
        let sequence = session.next_sequence;
        self.inner
            .store
            .begin_terminal_event(id, sequence, "close", &[])
            .await?;
        session.next_sequence += 1;
        let result: Result<_, RuntimeError> = async {
            let connection = self
                .inner
                .device(session.device_ref)
                .await
                .connection()
                .await?;
            Ok(connection.terminal_close(id, sequence).await?)
        }
        .await;
        self.inner
            .store
            .finish_terminal_event(
                id,
                sequence,
                if result.is_ok() {
                    "completed"
                } else {
                    "unconfirmed"
                },
            )
            .await?;
        result?;
        drop(session);
        for _ in 0..20 {
            match self.terminal_read(id).await {
                Ok(output) if output.ended => return Ok(()),
                Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                Err(error) => return Err(error),
            }
        }
        self.inner
            .store
            .finish_operation(id, "failed", Some("terminal did not finish after close"))
            .await?;
        self.inner.terminals.lock().await.remove(&id);
        Err(RuntimeError::Bridge(BridgeError::Terminal(
            "terminal did not finish after close".to_owned(),
        )))
    }

    async fn terminal_entry(
        &self,
        id: RequestId,
    ) -> Result<Arc<Mutex<TerminalRuntimeSession>>, RuntimeError> {
        self.inner
            .terminals
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or_else(|| {
                RuntimeError::Bridge(BridgeError::Terminal(
                    "terminal session is not active".to_owned(),
                ))
            })
    }
}

fn validate_terminal_size(cols: u16, rows: u16) -> Result<(), RuntimeError> {
    if !(20..=500).contains(&cols) || !(5..=200).contains(&rows) {
        return Err(RuntimeError::Bridge(BridgeError::Terminal(
            "terminal dimensions are outside supported bounds".to_owned(),
        )));
    }
    Ok(())
}
