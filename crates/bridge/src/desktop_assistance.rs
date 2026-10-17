//! Local handoff between an MCP process and this OS user's Desktop.
//! Account/service isolation comes from the existing operator data directory
//! and account revision. The unauthenticated reporting socket stays read-only.
use pab_protocol::DeviceCode;
use serde::{Deserialize, Serialize};
use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

const LEASE_MS: i64 = 30_000;
const RETAIN_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub const HEARTBEAT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("desktop assistance storage failed")]
    Database(#[from] sqlx::Error),
    #[error("invalid desktop assistance state")]
    Encoding(#[from] serde_json::Error),
    #[error("desktop assistance requires the current signed-in account")]
    Account,
    #[error("desktop assistance request was not found in this session")]
    NotFound,
    #[error("request_id already has different desktop assistance arguments")]
    Conflict,
    #[error("invalid desktop assistance request")]
    Invalid,
    #[error("desktop assistance participant is no longer active")]
    ParticipantGone,
    #[error("another Desktop instance is already handling this account")]
    DesktopBusy,
    #[error("desktop assistance must be active before completing it")]
    NotActive,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub request_id: Uuid,
    pub device_code: DeviceCode,
    pub reason: String,
    pub expected_action: String,
    pub screen_id: Option<String>,
    pub expires_in_seconds: u32,
}
impl Request {
    pub fn validate(&self) -> Result<(), Error> {
        if self.valid() {
            Ok(())
        } else {
            Err(Error::Invalid)
        }
    }
    fn valid(&self) -> bool {
        !self.request_id.is_nil()
            && !self.reason.trim().is_empty()
            && self.reason.len() <= 4096
            && !self.expected_action.trim().is_empty()
            && self.expected_action.len() <= 4096
            && self
                .screen_id
                .as_ref()
                .is_none_or(|s| !s.is_empty() && s.len() <= 1024)
            && (1..=86400).contains(&self.expires_in_seconds)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Waiting,
    Active,
    Completed,
    Cancelled,
    Expired,
}
impl Status {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Expired)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    DesktopNotRunning,
    Pending,
    Displayed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub assistance_id: Uuid,
    pub request: Request,
    pub status: Status,
    pub delivery: Delivery,
    pub created_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub revision: u64,
    /// A machine-readable outcome only, never typed input or completion text.
    pub outcome: Option<String>,
    pub window_label: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    owner: Uuid,
    desktop: Option<Uuid>,
    snapshot: Snapshot,
}
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    path: PathBuf,
    scope: String,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}
impl Store {
    pub async fn for_current_account() -> Result<Self, Error> {
        let (path, scope) = Self::current_identity()?;
        Self::open(&path, scope).await
    }
    fn current_identity() -> Result<(PathBuf, String), Error> {
        let account = pab_agent_core::account::AccountStore::from_env()
            .and_then(|s| s.read())
            .map_err(|_| Error::Account)?;
        let user = account.user.ok_or(Error::Account)?;
        let paths = pab_agent_core::DataPaths::for_operator().map_err(|_| Error::Account)?;
        pab_agent_core::ensure_data_dir(paths.root()).map_err(|_| Error::Account)?;
        Ok((
            paths.root().join("desktop-assistance.sqlite3"),
            format!("{}:{}", user.id, account.revision),
        ))
    }
    pub fn is_current_account(&self) -> bool {
        Self::current_identity().is_ok_and(|(path, scope)| path == self.path && scope == self.scope)
    }
    async fn open(path: &Path, scope: String) -> Result<Self, Error> {
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(Duration::from_secs(5)),
            )
            .await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS assistance_requests (id TEXT PRIMARY KEY, scope TEXT NOT NULL, owner TEXT NOT NULL, request_key TEXT NOT NULL, payload TEXT NOT NULL, terminal INTEGER NOT NULL, expires INTEGER NOT NULL, UNIQUE(scope, owner, request_key))").execute(&pool).await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS assistance_scope ON assistance_requests(scope, terminal)",
        )
        .execute(&pool)
        .await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS assistance_participants (scope TEXT NOT NULL, role TEXT NOT NULL, actor TEXT NOT NULL, lease_until INTEGER NOT NULL, PRIMARY KEY(scope, role, actor))").execute(&pool).await?;
        Ok(Self {
            pool,
            path: path.to_path_buf(),
            scope,
            clock: Arc::new(crate::desktop_presence::now_ms),
        })
    }
    async fn save(&self, tx: &mut Transaction<'_, Sqlite>, entry: &Entry) -> Result<(), Error> {
        sqlx::query("UPDATE assistance_requests SET payload=?,terminal=? WHERE scope=? AND id=?")
            .bind(serde_json::to_string(entry)?)
            .bind(entry.snapshot.status.terminal())
            .bind(&self.scope)
            .bind(entry.snapshot.assistance_id.to_string())
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    async fn load(&self, tx: &mut Transaction<'_, Sqlite>, id: Uuid) -> Result<Entry, Error> {
        let data: Option<String> =
            sqlx::query_scalar("SELECT payload FROM assistance_requests WHERE scope=? AND id=?")
                .bind(&self.scope)
                .bind(id.to_string())
                .fetch_optional(&mut **tx)
                .await?;
        Ok(serde_json::from_str(&data.ok_or(Error::NotFound)?)?)
    }
    async fn live(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        role: &str,
        actor: Uuid,
    ) -> Result<(), Error> {
        let present: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM assistance_participants WHERE scope=? AND role=? AND actor=? AND lease_until>?)")
            .bind(&self.scope).bind(role).bind(actor.to_string()).bind((self.clock)()).fetch_one(&mut **tx).await?;
        if present {
            Ok(())
        } else {
            Err(Error::ParticipantGone)
        }
    }
    async fn begin(&self) -> Result<Transaction<'_, Sqlite>, Error> {
        // Serialize maintenance and mutations across MCP/Desktop processes.
        // Deferred transactions can otherwise read a stale snapshot then fail
        // to upgrade their lock when two requests arrive simultaneously.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = (self.clock)();
        sqlx::query("DELETE FROM assistance_requests WHERE expires<?")
            .bind(now.saturating_sub(RETAIN_MS))
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM assistance_participants WHERE lease_until<=?")
            .bind(now)
            .execute(&mut *tx)
            .await?;
        let actors = sqlx::query("SELECT role,actor FROM assistance_participants WHERE scope=?")
            .bind(&self.scope)
            .fetch_all(&mut *tx)
            .await?;
        let owners: HashSet<String> = actors
            .iter()
            .filter(|r| r.get::<String, _>("role") == "requester")
            .map(|r| r.get("actor"))
            .collect();
        let desktop: Option<String> = actors
            .iter()
            .find(|r| r.get::<String, _>("role") == "desktop")
            .map(|r| r.get("actor"));
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT payload FROM assistance_requests WHERE scope=? AND terminal=0",
        )
        .bind(&self.scope)
        .fetch_all(&mut *tx)
        .await?;
        for data in rows {
            let mut entry: Entry = serde_json::from_str(&data)?;
            let previous = serde_json::to_string(&entry)?;
            let s = &mut entry.snapshot;
            if s.expires_at_unix_ms <= now {
                s.status = Status::Expired;
                s.outcome = Some("expired".into());
            } else if !owners.contains(&entry.owner.to_string()) {
                s.status = Status::Cancelled;
                s.outcome = Some("requester_unavailable".into());
            } else if desktop.is_none()
                || entry
                    .desktop
                    .is_some_and(|id| Some(id.to_string()) != desktop)
            {
                s.status = Status::Waiting;
                s.window_label = None;
                entry.desktop = None;
                s.delivery = if desktop.is_some() {
                    Delivery::Pending
                } else {
                    Delivery::DesktopNotRunning
                };
            } else if s.delivery == Delivery::DesktopNotRunning {
                s.delivery = Delivery::Pending;
            }
            if serde_json::to_string(&entry)? != previous {
                entry.snapshot.revision += 1;
                self.save(&mut tx, &entry).await?;
            }
        }
        Ok(tx)
    }
    pub async fn heartbeat_requester(&self, actor: Uuid) -> Result<(), Error> {
        self.heartbeat("requester", actor).await
    }
    pub async fn heartbeat_desktop(&self, actor: Uuid) -> Result<(), Error> {
        self.heartbeat("desktop", actor).await
    }
    async fn heartbeat(&self, role: &str, actor: Uuid) -> Result<(), Error> {
        if actor.is_nil() {
            return Err(Error::Invalid);
        }
        let mut tx = self.begin().await?;
        if role == "desktop" {
            let other: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM assistance_participants WHERE scope=? AND role='desktop' AND actor<>?)")
                .bind(&self.scope).bind(actor.to_string()).fetch_one(&mut *tx).await?;
            if other {
                return Err(Error::DesktopBusy);
            }
        }
        sqlx::query("INSERT INTO assistance_participants(scope,role,actor,lease_until) VALUES(?,?,?,?) ON CONFLICT(scope,role,actor) DO UPDATE SET lease_until=excluded.lease_until")
            .bind(&self.scope).bind(role).bind(actor.to_string()).bind((self.clock)().saturating_add(LEASE_MS)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn request(&self, owner: Uuid, request: Request) -> Result<Snapshot, Error> {
        if !request.valid() {
            return Err(Error::Invalid);
        }
        let mut tx = self.begin().await?;
        self.live(&mut tx, "requester", owner).await?;
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT payload FROM assistance_requests WHERE scope=? AND owner=? AND request_key=?",
        )
        .bind(&self.scope)
        .bind(owner.to_string())
        .bind(request.request_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let entry = if let Some(data) = existing {
            let entry: Entry = serde_json::from_str(&data)?;
            if entry.snapshot.request != request {
                return Err(Error::Conflict);
            }
            entry
        } else {
            let desktop: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM assistance_participants WHERE scope=? AND role='desktop')")
                .bind(&self.scope).fetch_one(&mut *tx).await?;
            let now = (self.clock)();
            let entry = Entry {
                owner,
                desktop: None,
                snapshot: Snapshot {
                    assistance_id: Uuid::new_v4(),
                    expires_at_unix_ms: now
                        .saturating_add(i64::from(request.expires_in_seconds) * 1000),
                    request,
                    status: Status::Waiting,
                    delivery: if desktop {
                        Delivery::Pending
                    } else {
                        Delivery::DesktopNotRunning
                    },
                    created_at_unix_ms: now,
                    revision: 1,
                    outcome: None,
                    window_label: None,
                },
            };
            sqlx::query("INSERT INTO assistance_requests(id,scope,owner,request_key,payload,terminal,expires) VALUES(?,?,?,?,?,0,?)")
                .bind(entry.snapshot.assistance_id.to_string()).bind(&self.scope).bind(owner.to_string())
                .bind(entry.snapshot.request.request_id.to_string()).bind(serde_json::to_string(&entry)?)
                .bind(entry.snapshot.expires_at_unix_ms).execute(&mut *tx).await?;
            entry
        };
        tx.commit().await?;
        Ok(entry.snapshot)
    }
    /// Check an idempotent request before reconnecting a device. A target
    /// going offline must not hide the outcome of an already accepted request.
    pub async fn replay(&self, owner: Uuid, request: &Request) -> Result<Option<Snapshot>, Error> {
        request.validate()?;
        let mut tx = self.begin().await?;
        let data: Option<String> = sqlx::query_scalar(
            "SELECT payload FROM assistance_requests WHERE scope=? AND owner=? AND request_key=?",
        )
        .bind(&self.scope)
        .bind(owner.to_string())
        .bind(request.request_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let result = if let Some(data) = data {
            let entry: Entry = serde_json::from_str(&data)?;
            if &entry.snapshot.request != request {
                return Err(Error::Conflict);
            }
            Some(entry.snapshot)
        } else {
            None
        };
        tx.commit().await?;
        Ok(result)
    }
    pub async fn status(&self, owner: Uuid, id: Uuid) -> Result<Snapshot, Error> {
        let mut tx = self.begin().await?;
        let entry = self.load(&mut tx, id).await?;
        if entry.owner != owner {
            return Err(Error::NotFound);
        }
        tx.commit().await?;
        Ok(entry.snapshot)
    }
    pub async fn cancel(&self, owner: Uuid, id: Uuid) -> Result<Snapshot, Error> {
        let mut tx = self.begin().await?;
        let mut entry = self.load(&mut tx, id).await?;
        if entry.owner != owner {
            return Err(Error::NotFound);
        }
        if !entry.snapshot.status.terminal() {
            entry.snapshot.status = Status::Cancelled;
            entry.snapshot.outcome = Some("requester_cancelled".into());
            entry.snapshot.revision += 1;
            self.save(&mut tx, &entry).await?;
        }
        tx.commit().await?;
        Ok(entry.snapshot)
    }
    pub async fn pending(&self, desktop: Uuid) -> Result<Vec<Snapshot>, Error> {
        let mut tx = self.begin().await?;
        self.live(&mut tx, "desktop", desktop).await?;
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT payload FROM assistance_requests WHERE scope=? AND terminal=0 ORDER BY rowid",
        )
        .bind(&self.scope)
        .fetch_all(&mut *tx)
        .await?;
        let values = rows
            .iter()
            .map(|s| serde_json::from_str::<Entry>(s).map(|e| e.snapshot))
            .collect::<Result<_, _>>()?;
        tx.commit().await?;
        Ok(values)
    }
    /// Called only after the prompt is actually mounted in the Desktop UI.
    pub async fn displayed(&self, desktop: Uuid, id: Uuid) -> Result<Snapshot, Error> {
        self.ui_change(desktop, id, None, false, false).await
    }
    /// The caller must verify that this window is showing the requested device.
    /// A created window or successful connect call alone is not activation.
    pub async fn activate(&self, desktop: Uuid, id: Uuid, window: &str) -> Result<Snapshot, Error> {
        if !window.starts_with("desktop-viewer-") || window.len() > 128 {
            return Err(Error::Invalid);
        }
        self.ui_change(desktop, id, Some(window), false, false)
            .await
    }
    /// Only an explicit user action may call this. No completion text is stored.
    pub async fn complete(&self, desktop: Uuid, id: Uuid) -> Result<Snapshot, Error> {
        self.ui_change(desktop, id, None, true, false).await
    }
    pub async fn dismiss(&self, desktop: Uuid, id: Uuid) -> Result<Snapshot, Error> {
        self.ui_change(desktop, id, None, false, true).await
    }
    async fn ui_change(
        &self,
        desktop: Uuid,
        id: Uuid,
        window: Option<&str>,
        complete: bool,
        cancel: bool,
    ) -> Result<Snapshot, Error> {
        let mut tx = self.begin().await?;
        self.live(&mut tx, "desktop", desktop).await?;
        let mut entry = self.load(&mut tx, id).await?;
        let previous = serde_json::to_string(&entry)?;
        if !entry.snapshot.status.terminal() {
            if complete && entry.snapshot.status != Status::Active {
                return Err(Error::NotActive);
            }
            entry.desktop = Some(desktop);
            entry.snapshot.delivery = Delivery::Displayed;
            if let Some(window) = window {
                entry.snapshot.window_label = Some(window.into());
                entry.snapshot.status = Status::Active;
            }
            if complete {
                entry.snapshot.status = Status::Completed;
                entry.snapshot.outcome = Some("user_completed".into());
            }
            if cancel {
                entry.snapshot.status = Status::Cancelled;
                entry.snapshot.outcome = Some("user_cancelled".into());
            }
            if serde_json::to_string(&entry)? != previous {
                entry.snapshot.revision += 1;
                self.save(&mut tx, &entry).await?;
            }
        }
        tx.commit().await?;
        Ok(entry.snapshot)
    }
    pub async fn window_closed(&self, desktop: Uuid, window: &str) -> Result<(), Error> {
        let mut tx = self.begin().await?;
        self.live(&mut tx, "desktop", desktop).await?;
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT payload FROM assistance_requests WHERE scope=? AND terminal=0",
        )
        .bind(&self.scope)
        .fetch_all(&mut *tx)
        .await?;
        for data in rows {
            let mut entry: Entry = serde_json::from_str(&data)?;
            if entry.desktop == Some(desktop)
                && entry.snapshot.window_label.as_deref() == Some(window)
            {
                entry.snapshot.status = Status::Waiting;
                entry.snapshot.window_label = None;
                entry.snapshot.revision += 1;
                self.save(&mut tx, &entry).await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn stop_requester(&self, owner: Uuid) -> Result<(), Error> {
        self.stop("requester", owner).await
    }
    pub async fn stop_desktop(&self, desktop: Uuid) -> Result<(), Error> {
        self.stop("desktop", desktop).await
    }
    async fn stop(&self, role: &str, actor: Uuid) -> Result<(), Error> {
        sqlx::query("DELETE FROM assistance_participants WHERE scope=? AND role=? AND actor=?")
            .bind(&self.scope)
            .bind(role)
            .bind(actor.to_string())
            .execute(&self.pool)
            .await?;
        self.begin().await?.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "desktop_assistance_tests.rs"]
mod tests;
