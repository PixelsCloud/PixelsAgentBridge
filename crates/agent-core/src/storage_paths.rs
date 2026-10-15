use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataScope {
    User,
    Machine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataPaths {
    root: PathBuf,
}

impl DataPaths {
    pub fn for_scope(scope: DataScope) -> Result<Self, DataPathError> {
        Ok(Self {
            root: persistent_data_dir(scope)?,
        })
    }

    pub fn for_scope_with(
        scope: DataScope,
        lookup: impl FnMut(&str) -> Option<OsString>,
    ) -> Result<Self, DataPathError> {
        Ok(Self {
            root: persistent_data_dir_with(scope, lookup)?,
        })
    }

    pub fn from_root(root: PathBuf) -> Result<Self, DataPathError> {
        if root.as_os_str().is_empty() {
            return Err(DataPathError::EmptyOverride);
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Operator credentials, guest endpoints and task history belong to one
    /// control service. Never open a different service's database or resume its
    /// queued work. Settings, account stores and local service IPC stay at root.
    pub fn for_operator() -> Result<Self, DataPathError> {
        let paths = Self::for_scope(DataScope::User)?;
        match env::var("PAB_CONTROL_URL") {
            Ok(server) if !server.is_empty() => paths.operator_for_server(&server),
            _ => Self::from_root(paths.root.join("operators").join("unconfigured")),
        }
    }

    pub fn operator_for_server(&self, server: &str) -> Result<Self, DataPathError> {
        let mut url = url::Url::parse(server).map_err(|_| DataPathError::InvalidServer)?;
        if url.scheme() != "wss"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(DataPathError::InvalidServer);
        }
        // URL parsing normalizes host casing and the default TLS port.
        url.set_fragment(None);
        let scope = blake3::hash(url.as_str().as_bytes()).to_hex().to_string();
        Self::from_root(self.root.join("operators").join(scope))
    }

    pub fn bridge_endpoint_secret(&self) -> PathBuf {
        self.root.join("bridge-endpoint.key")
    }

    pub fn bridge_database(&self) -> PathBuf {
        self.root.join("bridge.sqlite3")
    }

    pub fn executor_endpoint_secret(&self) -> PathBuf {
        self.root.join("device-endpoint.key")
    }

    pub fn executor_database(&self) -> PathBuf {
        self.root.join("executor.sqlite3")
    }
}

pub fn ensure_data_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

pub fn ensure_data_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        ensure_data_dir(parent)?;
    }
    Ok(())
}

/// Data lives outside the application installation directory so reinstalling
/// binaries does not replace endpoint identities or local task history.
pub fn persistent_data_dir(scope: DataScope) -> Result<PathBuf, DataPathError> {
    persistent_data_dir_with(scope, |name| env::var_os(name))
}

fn persistent_data_dir_with(
    scope: DataScope,
    lookup: impl FnMut(&str) -> Option<OsString>,
) -> Result<PathBuf, DataPathError> {
    persistent_data_dir_for_os(env::consts::OS, scope, lookup)
}

fn persistent_data_dir_for_os(
    os: &str,
    scope: DataScope,
    mut lookup: impl FnMut(&str) -> Option<OsString>,
) -> Result<PathBuf, DataPathError> {
    if let Some(override_dir) = lookup("PAB_DATA_DIR") {
        let path = PathBuf::from(override_dir);
        return (!path.as_os_str().is_empty())
            .then_some(path)
            .ok_or(DataPathError::EmptyOverride);
    }

    match os {
        "windows" => {
            let variable = match scope {
                DataScope::User => "LOCALAPPDATA",
                DataScope::Machine => "PROGRAMDATA",
            };
            lookup(variable)
                .filter(|value| !value.is_empty())
                .map(|root| PathBuf::from(root).join("PixelsAgentBridge"))
                .ok_or(DataPathError::MissingBase(variable))
        }
        "macos" => match scope {
            DataScope::User => lookup("HOME")
                .filter(|value| !value.is_empty())
                .map(|root| {
                    PathBuf::from(root).join("Library/Application Support/PixelsAgentBridge")
                })
                .ok_or(DataPathError::MissingBase("HOME")),
            DataScope::Machine => Ok(PathBuf::from(
                "/Library/Application Support/PixelsAgentBridgeData",
            )),
        },
        "linux" => match scope {
            DataScope::User => match lookup("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
                Some(root) => Ok(PathBuf::from(root).join("pixels-agent-bridge")),
                None => lookup("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|root| PathBuf::from(root).join(".local/share/pixels-agent-bridge"))
                    .ok_or(DataPathError::MissingBase("HOME")),
            },
            DataScope::Machine => Ok(PathBuf::from("/var/lib/pixels-agent-bridge")),
        },
        _ => Err(DataPathError::UnsupportedPlatform),
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DataPathError {
    #[error("operator server must be a valid WSS URL")]
    InvalidServer,
    #[error("PAB_DATA_DIR must not be empty")]
    EmptyOverride,
    #[error("{0} is required to find the persistent data directory")]
    MissingBase(&'static str),
    #[error("this platform has no persistent data directory mapping")]
    UnsupportedPlatform,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_servers_never_share_device_passwords_history_or_guest_keys() {
        let root = DataPaths::from_root(PathBuf::from("user-data")).unwrap();
        let a = root
            .operator_for_server("wss://one.invalid/control")
            .unwrap();
        let b = root
            .operator_for_server("wss://two.invalid/control")
            .unwrap();
        assert_ne!(a.bridge_database(), b.bridge_database());
        assert_ne!(a.bridge_endpoint_secret(), b.bridge_endpoint_secret());
        assert_ne!(a.bridge_database(), root.bridge_database());
        assert_eq!(
            a,
            root.operator_for_server("wss://ONE.invalid:443/control")
                .unwrap()
        );
        assert_ne!(
            a,
            root.operator_for_server("wss://one.invalid/other").unwrap()
        );
        assert!(root.operator_for_server("http://one.invalid").is_err());
        assert!(
            root.operator_for_server("wss://user:password@one.invalid")
                .is_err()
        );
    }

    #[test]
    fn override_is_used_for_both_scopes() {
        for scope in [DataScope::User, DataScope::Machine] {
            assert_eq!(
                persistent_data_dir_with(scope, |name| {
                    (name == "PAB_DATA_DIR").then(|| OsString::from("persistent-data"))
                })
                .unwrap(),
                PathBuf::from("persistent-data")
            );
        }
    }

    #[test]
    fn empty_override_is_rejected() {
        assert_eq!(
            persistent_data_dir_with(DataScope::User, |name| {
                (name == "PAB_DATA_DIR").then(OsString::new)
            })
            .unwrap_err(),
            DataPathError::EmptyOverride
        );
    }

    #[test]
    fn standard_paths_share_one_persistent_root() {
        let paths = DataPaths::from_root(PathBuf::from("data-root")).unwrap();
        assert_eq!(
            paths.bridge_database(),
            PathBuf::from("data-root/bridge.sqlite3")
        );
        assert_eq!(
            paths.executor_database(),
            PathBuf::from("data-root/executor.sqlite3")
        );
        assert_eq!(
            paths.executor_endpoint_secret(),
            PathBuf::from("data-root/device-endpoint.key")
        );
    }

    #[test]
    fn windows_user_and_machine_roots_are_separate() {
        let lookup = |name: &str| match name {
            "LOCALAPPDATA" => Some(OsString::from(r"C:\Users\Tester\AppData\Local")),
            "PROGRAMDATA" => Some(OsString::from(r"C:\ProgramData")),
            _ => None,
        };
        assert_eq!(
            persistent_data_dir_for_os("windows", DataScope::User, lookup).unwrap(),
            PathBuf::from(r"C:\Users\Tester\AppData\Local").join("PixelsAgentBridge")
        );
        assert_eq!(
            persistent_data_dir_for_os("windows", DataScope::Machine, lookup).unwrap(),
            PathBuf::from(r"C:\ProgramData").join("PixelsAgentBridge")
        );
    }

    #[test]
    fn macos_uses_application_support() {
        let lookup = |name: &str| (name == "HOME").then(|| OsString::from("/Users/tester"));
        assert_eq!(
            persistent_data_dir_for_os("macos", DataScope::User, lookup).unwrap(),
            PathBuf::from("/Users/tester").join("Library/Application Support/PixelsAgentBridge")
        );
        assert_eq!(
            persistent_data_dir_for_os("macos", DataScope::Machine, lookup).unwrap(),
            PathBuf::from("/Library/Application Support/PixelsAgentBridgeData")
        );
    }

    #[test]
    fn linux_honors_xdg_data_home() {
        let lookup = |name: &str| match name {
            "XDG_DATA_HOME" => Some(OsString::from("/home/tester/custom-data")),
            "HOME" => Some(OsString::from("/home/tester")),
            _ => None,
        };
        assert_eq!(
            persistent_data_dir_for_os("linux", DataScope::User, lookup).unwrap(),
            PathBuf::from("/home/tester/custom-data").join("pixels-agent-bridge")
        );
        assert_eq!(
            persistent_data_dir_for_os("linux", DataScope::Machine, lookup).unwrap(),
            PathBuf::from("/var/lib/pixels-agent-bridge")
        );
    }
}
