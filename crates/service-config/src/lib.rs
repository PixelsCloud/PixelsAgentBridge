//! File configuration helpers for Server and Relay only.
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read configuration file: {0}")]
    Read(std::io::Error),
    // Never format the TOML error: it can include credentials from the source.
    #[error("invalid configuration at byte {offset}; check TOML syntax, field names and types")]
    Parse { offset: usize },
    #[error("{0}")]
    Invalid(&'static str),
}
pub fn load<T: DeserializeOwned>(path: &Path) -> Result<T, ConfigError> {
    let raw = std::fs::read_to_string(path).map_err(ConfigError::Read)?;
    toml::from_str(raw.trim_start_matches('\u{feff}')).map_err(|e: toml::de::Error| {
        ConfigError::Parse {
            offset: e.span().map_or(0, |s| s.start),
        }
    })
}
pub fn resolve(base: &Path, path: &mut PathBuf) {
    if path.is_relative() {
        *path = base.join(&*path);
    }
}
/// --config may precede or follow the subcommand. All other arguments remain intact.
pub fn arguments(
    default: &str,
    args: impl IntoIterator<Item = OsString>,
) -> Result<(PathBuf, Vec<String>), ConfigError> {
    let mut config = None;
    let mut rest = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--config" {
            if config.is_some() {
                return Err(ConfigError::Invalid("--config must appear once"));
            }
            let value = args
                .next()
                .filter(|v| !v.is_empty() && !v.to_string_lossy().starts_with("--"))
                .ok_or(ConfigError::Invalid("--config requires a file path"))?;
            config = Some(PathBuf::from(value));
        } else {
            rest.push(
                arg.into_string()
                    .map_err(|_| ConfigError::Invalid("invalid command argument"))?,
            );
        }
    }
    Ok((config.unwrap_or_else(|| default.into()), rest))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsConfig {
    pub cert: PathBuf,
    pub key: PathBuf,
}
impl TlsConfig {
    pub fn resolve(&mut self, base: &Path) {
        resolve(base, &mut self.cert);
        resolve(base, &mut self.key);
    }
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    pub directory: PathBuf,
    pub level: String,
}
impl Default for LogConfig {
    fn default() -> Self {
        Self {
            directory: "logs".into(),
            level: "info".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arguments_accept_both_positions_and_reject_ambiguous_paths() {
        for args in [
            vec!["serve", "--config", "配置.toml"],
            vec!["--config", "配置.toml", "serve"],
        ] {
            let (path, rest) =
                arguments("default.toml", args.into_iter().map(OsString::from)).unwrap();
            assert_eq!(path, PathBuf::from("配置.toml"));
            assert_eq!(rest, ["serve"]);
        }
        for args in [
            vec!["--config"],
            vec!["--config", "a", "--config", "b"],
            vec!["--config", "--bad"],
        ] {
            assert!(arguments("default.toml", args.into_iter().map(OsString::from)).is_err());
        }
    }
    #[test]
    fn errors_never_contain_toml_values_or_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private.toml");
        for content in [
            "private-secret = 'password'",
            "level = ['password']",
            "level = 'password",
        ] {
            std::fs::write(&path, content).unwrap();
            let err = load::<LogConfig>(&path).err().unwrap().to_string();
            assert!(!err.contains("password") && !err.contains("private-secret"));
        }
        std::fs::write(&path, "\u{feff}level = 'warn'").unwrap();
        assert_eq!(load::<LogConfig>(&path).unwrap().level, "warn");
    }
}
