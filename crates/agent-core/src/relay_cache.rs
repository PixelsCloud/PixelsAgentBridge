//! Bounded runtime cache; it never grants authorization or replaces control policy.
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

#[derive(Clone)]
pub(crate) struct RelayCache {
    pub path: PathBuf,
    pub scope: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    schema: u32,
    scope: String,
    written_at: u64,
    urls: Vec<String>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn valid(urls: &[String]) -> bool {
    urls.len() <= 64
        && urls.iter().all(|value| {
            url::Url::parse(value).is_ok_and(|url| {
                url.scheme() == "https"
                    && url.host_str().is_some()
                    && url.path() == "/"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none()
            })
        })
}
impl RelayCache {
    pub fn load(&self) -> Option<Vec<String>> {
        let mut bytes = Vec::new();
        std::fs::File::open(&self.path)
            .ok()?
            .take(65537)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > 65536 {
            return None;
        }
        let data: Snapshot = serde_json::from_slice(&bytes).ok()?;
        (data.schema == 1
            && data.scope == self.scope
            && data.written_at <= now()
            && now().saturating_sub(data.written_at) < 86400
            && valid(&data.urls))
        .then_some(data.urls)
    }
    fn write(&self, urls: Vec<String>) -> std::io::Result<()> {
        if !valid(&urls) {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        let data = Snapshot {
            schema: 1,
            scope: self.scope.clone(),
            written_at: now(),
            urls,
        };
        let parent = self.path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
        std::fs::create_dir_all(parent)?;
        let temp = self
            .path
            .with_extension(format!("{}.tmp", pab_protocol::RequestId::new()));
        let result = (|| {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(&serde_json::to_vec(&data)?)?;
            file.sync_all()?;
            // std::fs::rename replaces an existing file on supported Windows and Unix targets.
            std::fs::rename(&temp, &self.path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }
    pub fn writer(self) -> watch::Sender<Option<Vec<String>>> {
        let (sender, mut receiver) = watch::channel(None::<Vec<String>>);
        tokio::spawn(async move {
            while receiver.changed().await.is_ok() {
                let Some(urls) = receiver.borrow_and_update().clone() else {
                    continue;
                };
                let cache = self.clone();
                // Blocking disk work cannot delay heartbeats or grow an unbounded write queue.
                let _ = tokio::task::spawn_blocking(move || cache.write(urls)).await;
                // Coalesce confirmations/changes into at most one disk write per
                // minute; the latest complete snapshot remains in the watch slot.
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        });
        sender
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_is_bounded_scoped_and_atomic() {
        let root = tempfile::tempdir().unwrap();
        let cache = RelayCache {
            path: root.path().join("cache.json"),
            scope: "a".into(),
        };
        cache.write(vec!["https://a.example/".into()]).unwrap();
        cache.write(vec!["https://b.example/".into()]).unwrap();
        assert_eq!(cache.load().unwrap(), ["https://b.example/"]);
        assert!(
            RelayCache {
                scope: "b".into(),
                ..cache.clone()
            }
            .load()
            .is_none()
        );
        assert!(cache.write(vec!["http://a.example/".into()]).is_err());
        std::fs::write(&cache.path, vec![b' '; 65537]).unwrap();
        assert!(cache.load().is_none());
        let old = Snapshot {
            schema: 1,
            scope: "a".into(),
            written_at: now() - 86401,
            urls: vec![],
        };
        std::fs::write(&cache.path, serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(cache.load().is_none());
    }
}
