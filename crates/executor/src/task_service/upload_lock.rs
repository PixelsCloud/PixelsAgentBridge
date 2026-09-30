use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub(super) struct UploadPathLocks {
    active: Arc<Mutex<HashSet<PathBuf>>>,
}

impl UploadPathLocks {
    pub(super) async fn try_acquire(
        &self,
        destination: &Path,
    ) -> io::Result<Option<UploadPathGuard>> {
        let parent = destination
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing parent path"))?;
        let canonical_parent = tokio::fs::canonicalize(parent).await?;
        let file_name = destination
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing file name"))?;
        let key = canonical_parent.join(file_name);
        #[cfg(windows)]
        let key = PathBuf::from(key.to_string_lossy().to_lowercase());

        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // A directory operation excludes concurrent writers anywhere beneath it.
        if active
            .iter()
            .any(|held| held.starts_with(&key) || key.starts_with(held))
        {
            return Ok(None);
        }
        active.insert(key.clone());
        Ok(Some(UploadPathGuard {
            active: Arc::clone(&self.active),
            key,
        }))
    }
}

pub(super) struct UploadPathGuard {
    active: Arc<Mutex<HashSet<PathBuf>>>,
    key: PathBuf,
}

impl Drop for UploadPathGuard {
    fn drop(&mut self) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        active.remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_uploads_to_one_destination_conflict_until_first_ends() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("shared.bin");
        let other = directory.path().join("other.bin");
        let locks = UploadPathLocks::default();
        let first = locks.try_acquire(&destination).await.unwrap().unwrap();
        assert!(locks.try_acquire(&destination).await.unwrap().is_none());
        assert!(locks.try_acquire(&other).await.unwrap().is_some());
        drop(first);
        assert!(locks.try_acquire(&destination).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn directory_locks_exclude_children_in_both_directions_and_normalize_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("tree");
        tokio::fs::create_dir(&root).await.unwrap();
        let child = root.join("child");
        let locks = UploadPathLocks::default();
        let guard = locks.try_acquire(&root).await.unwrap().unwrap();
        assert!(locks.try_acquire(&child).await.unwrap().is_none());
        assert!(
            locks
                .try_acquire(&dir.path().join("other"))
                .await
                .unwrap()
                .is_some()
        );
        drop(guard);
        let guard = locks.try_acquire(&child).await.unwrap().unwrap();
        assert!(locks.try_acquire(&root).await.unwrap().is_none());
        let alias = root.join("..").join("tree").join("child");
        assert!(locks.try_acquire(&alias).await.unwrap().is_none());
        #[cfg(windows)]
        assert!(
            locks
                .try_acquire(&root.join("CHILD"))
                .await
                .unwrap()
                .is_none()
        );
        drop(guard);
        assert!(locks.try_acquire(&root).await.unwrap().is_some());
    }
}
