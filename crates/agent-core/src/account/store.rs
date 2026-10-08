use super::{AccountError, AccountSession, AccountUser, account_origin};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountState {
    pub revision: u64,
    pub user: Option<AccountUser>,
    pub active_slot: Option<String>,
    pub pending_logouts: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_persist_across_store_instances_and_logout_never_restores_identity() {
        let directory = tempfile::tempdir().unwrap();
        let origin = format!("https://{}.invalid", pab_protocol::RequestId::new());
        let store = AccountStore::new(directory.path(), &origin).unwrap();
        let user = AccountUser {
            id: pab_protocol::UserId::new(),
            username: "account-store-test".into(),
            server_admin: false,
        };
        let first = store
            .login(AccountSession {
                user: user.clone(),
                access_token: "a".repeat(64),
            })
            .unwrap();
        let restored = AccountStore::new(directory.path(), &origin).unwrap();
        assert_eq!(restored.read().unwrap().revision, first.revision);
        assert_eq!(restored.read().unwrap().user, Some(user.clone()));
        let slot = first.active_slot.unwrap();
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&store.directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(
                    store
                        .directory
                        .join(format!("{}.{slot}.secret", store.scope))
                )
                .unwrap()
                .permissions()
                .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(
            restored.token(&slot).unwrap().unwrap().as_str(),
            "a".repeat(64)
        );
        assert!(
            !String::from_utf8(fs::read(store.path()).unwrap())
                .unwrap()
                .contains(&"a".repeat(64))
        );
        let guest = restored.logout().unwrap();
        assert!(guest.user.is_none() && guest.active_slot.is_none());
        assert!(guest.revision > first.revision);
        let second = store
            .login(AccountSession {
                user,
                access_token: "b".repeat(64),
            })
            .unwrap();
        restored.finish_logout(&slot).unwrap();
        assert_eq!(restored.read().unwrap().revision, second.revision);
        assert!(restored.read().unwrap().user.is_some());
        assert!(store.token(&slot).unwrap().is_none());
        let second_slot = second.active_slot.unwrap();
        assert!(store.finish_logout(&second_slot).is_err());
        store.logout().unwrap();
        store.finish_logout(&second_slot).unwrap();
    }

    #[test]
    fn namespaces_and_slot_validation_prevent_cross_server_or_path_access() {
        let directory = tempfile::tempdir().unwrap();
        let a = AccountStore::new(directory.path(), "https://a.invalid").unwrap();
        let b = AccountStore::new(directory.path(), "https://b.invalid").unwrap();
        a.logout().unwrap();
        assert_eq!(b.read().unwrap().revision, 0);
        assert!(a.token("../../anything").is_err());
    }
}

#[derive(Clone)]
pub struct AccountStore {
    directory: PathBuf,
    scope: String,
}

impl AccountStore {
    pub fn new(root: &Path, server: &str) -> Result<Self, AccountError> {
        let directory = root.join("accounts");
        fs::create_dir_all(&directory).map_err(|_| AccountError::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .map_err(|_| AccountError::Storage)?;
        }
        Ok(Self {
            directory,
            scope: blake3::hash(account_origin(server)?.as_bytes())
                .to_hex()
                .to_string(),
        })
    }

    pub fn from_env() -> Result<Self, AccountError> {
        let paths = crate::DataPaths::for_scope(crate::DataScope::User)
            .map_err(|_| AccountError::Storage)?;
        let server = std::env::var("PAB_CONTROL_URL").map_err(|_| AccountError::InvalidUrl)?;
        Self::new(paths.root(), &server)
    }

    fn path(&self) -> PathBuf {
        self.directory.join(format!("{}.json", self.scope))
    }

    pub fn read(&self) -> Result<AccountState, AccountError> {
        match fs::read(self.path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| AccountError::Storage),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(AccountState::default())
            }
            Err(_) => Err(AccountError::Storage),
        }
    }

    // File locking serializes Desktop, CLI and multiple processes. The revision
    // and pending logout queue survive crashes and never contain access tokens.
    fn change(
        &self,
        update: impl FnOnce(&mut AccountState) -> Result<(), AccountError>,
    ) -> Result<AccountState, AccountError> {
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.directory.join(format!("{}.lock", self.scope)))
            .map_err(|_| AccountError::Storage)?;
        lock.lock().map_err(|_| AccountError::Storage)?;
        let mut state = self.read()?;
        update(&mut state)?;
        let temporary = self.directory.join(format!(
            "{}.{}.tmp",
            self.scope,
            pab_protocol::RequestId::new()
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options
                .open(&temporary)
                .map_err(|_| AccountError::Storage)?;
            file.write_all(&serde_json::to_vec(&state).map_err(|_| AccountError::Storage)?)
                .map_err(|_| AccountError::Storage)?;
            file.sync_all().map_err(|_| AccountError::Storage)?;
            drop(file);
            fs::rename(&temporary, self.path()).map_err(|_| AccountError::Storage)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        Ok(state)
    }

    pub fn login(&self, session: AccountSession) -> Result<AccountState, AccountError> {
        let slot = pab_protocol::RequestId::new().to_string();
        let token = Zeroizing::new(session.access_token);
        self.write_token(&slot, &token)?;
        let result = self.change(|state| {
            if let Some(previous) = state.active_slot.take() {
                state.pending_logouts.push(previous);
            }
            state.revision = state.revision.checked_add(1).ok_or(AccountError::Storage)?;
            state.user = Some(session.user);
            state.active_slot = Some(slot.clone());
            Ok(())
        });
        if result.is_err() {
            let _ = self.delete_token(&slot);
        }
        result
    }

    pub fn logout(&self) -> Result<AccountState, AccountError> {
        self.change(|state| {
            if let Some(previous) = state.active_slot.take() {
                state.pending_logouts.push(previous);
            }
            state.revision = state.revision.checked_add(1).ok_or(AccountError::Storage)?;
            state.user = None;
            Ok(())
        })
    }

    pub fn finish_logout(&self, slot: &str) -> Result<(), AccountError> {
        self.change(|state| {
            if state.active_slot.as_deref() == Some(slot) {
                return Err(AccountError::Storage);
            }
            self.delete_token(slot)?;
            state.pending_logouts.retain(|pending| pending != slot);
            Ok(())
        })?;
        Ok(())
    }

    fn valid_slot(slot: &str) -> Result<(), AccountError> {
        slot.parse::<pab_protocol::RequestId>()
            .map(|_| ())
            .map_err(|_| AccountError::Storage)
    }

    #[cfg(windows)]
    fn entry(&self, slot: &str) -> Result<keyring::Entry, AccountError> {
        Self::valid_slot(slot)?;
        keyring::Entry::new(
            "PixelsAgentBridge.Account",
            &format!("{}:{slot}", self.scope),
        )
        .map_err(|_| AccountError::Storage)
    }

    pub fn token(&self, slot: &str) -> Result<Option<Zeroizing<String>>, AccountError> {
        Self::valid_slot(slot)?;
        #[cfg(target_os = "macos")]
        {
            super::macos_credentials::read(&self.scope, slot)
        }
        #[cfg(windows)]
        {
            match self.entry(slot)?.get_password() {
                Ok(token) => Ok(Some(Zeroizing::new(token))),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(_) => Err(AccountError::Storage),
            }
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            match fs::read_to_string(self.directory.join(format!("{}.{slot}.secret", self.scope))) {
                Ok(token) => Ok(Some(Zeroizing::new(token))),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(_) => Err(AccountError::Storage),
            }
        }
    }

    fn write_token(&self, slot: &str, token: &str) -> Result<(), AccountError> {
        Self::valid_slot(slot)?;
        #[cfg(windows)]
        {
            self.entry(slot)?
                .set_password(token)
                .map_err(|_| AccountError::Storage)?;
            Ok(())
        }
        #[cfg(target_os = "macos")]
        {
            super::macos_credentials::write(&self.scope, slot, token)
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(self.directory.join(format!("{}.{slot}.secret", self.scope)))
                .map_err(|_| AccountError::Storage)?;
            file.write_all(token.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(|_| AccountError::Storage)
        }
    }

    fn delete_token(&self, slot: &str) -> Result<(), AccountError> {
        Self::valid_slot(slot)?;
        #[cfg(target_os = "macos")]
        {
            super::macos_credentials::delete(&self.scope, slot)
        }
        #[cfg(windows)]
        {
            match self.entry(slot)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err(AccountError::Storage),
            }
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            match fs::remove_file(self.directory.join(format!("{}.{slot}.secret", self.scope))) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(AccountError::Storage),
            }
        }
    }
}
