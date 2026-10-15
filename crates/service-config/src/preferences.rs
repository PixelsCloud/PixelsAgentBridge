//! User preferences use TOML; JSON is read only for the one-time migration.
use serde::{Serialize, de::DeserializeOwned};
use std::{io::Write, path::Path};

pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = toml::to_string_pretty(value).map_err(|_| "Cannot serialize configuration")?;
    write(path, text.as_bytes(), false)
}

pub(crate) fn write(path: &Path, bytes: &[u8], create_only: bool) -> Result<(), String> {
    let parent = path.parent().ok_or("Configuration path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    if create_only {
        file.persist_noclobber(path).map_err(|e| e.to_string())?;
    } else {
        file.persist(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn load<T: DeserializeOwned + Serialize>(
    path: &Path,
    validate: impl Fn(&mut T) -> Result<(), String>,
) -> Result<Option<T>, String> {
    load_with_limit(path, validate, 65536)
}

pub fn load_with_limit<T: DeserializeOwned + Serialize>(
    path: &Path,
    validate: impl Fn(&mut T) -> Result<(), String>,
    limit: usize,
) -> Result<Option<T>, String> {
    let legacy = path.with_extension("json");
    let (source, migrating) = if path.try_exists().map_err(|e| e.to_string())? {
        (path, false)
    } else if legacy.try_exists().map_err(|e| e.to_string())? {
        (legacy.as_path(), true)
    } else {
        return Ok(None);
    };
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(source)
        .map_err(|e| e.to_string())?
        .take(limit as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|_| "Cannot read configuration")?;
    if text.len() > limit {
        return Err(format!("Configuration exceeds {limit} bytes"));
    }
    let text = text.trim_start_matches('\u{feff}');
    let mut value: T = if migrating {
        serde_json::from_str(text).map_err(|_| "Invalid legacy configuration")?
    } else {
        toml::from_str(text).map_err(|_| "Invalid TOML configuration")?
    };
    validate(&mut value)?;
    if migrating {
        let text = toml::to_string_pretty(&value).map_err(|_| "Cannot serialize configuration")?;
        // Another process may migrate at the same time. Never overwrite its file.
        if let Err(error) = write(path, text.as_bytes(), true) {
            if path.try_exists().map_err(|e| e.to_string())? {
                return load_with_limit(path, validate, limit);
            }
            return Err(error);
        }
        // Retain the original as a backup. It is never read again once TOML exists.
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug, serde::Deserialize, serde::Serialize)]
    #[serde(deny_unknown_fields)]
    struct Settings {
        enabled: bool,
    }

    #[test]
    fn legacy_migration_is_atomic_and_toml_takes_precedence() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.toml");
        let legacy = path.with_extension("json");
        std::fs::write(&legacy, "\u{feff}{\"enabled\":true}").unwrap();
        assert!(
            load::<Settings>(&path, |_| Ok(()))
                .unwrap()
                .unwrap()
                .enabled
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("enabled = true")
        );
        save(&path, &Settings { enabled: false }).unwrap();
        assert!(
            !load::<Settings>(&path, |_| Ok(()))
                .unwrap()
                .unwrap()
                .enabled
        );
        // Invalid TOML must fail, rather than silently falling back to stale JSON.
        std::fs::write(&path, "enabled = 'secret-password'").unwrap();
        let error = load::<Settings>(&path, |_| Ok(())).unwrap_err();
        assert!(!error.contains("secret-password"));
        std::fs::remove_file(&path).unwrap();
        assert!(load::<Settings>(&path, |_| Err("Invalid settings".into())).is_err());
        assert!(!path.exists());
    }
}
