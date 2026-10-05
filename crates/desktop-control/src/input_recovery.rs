//! Small, locked write-ahead bitmap. Stores only outstanding releases, never text.
//! A live owner holds the OS lock; process death releases it for the next helper.
use std::{
    fs::{File, OpenOptions, TryLockError},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

pub(crate) const KEY_SLOTS: usize = 256;
pub(crate) const SLOTS: usize = KEY_SLOTS + 3;

pub(crate) struct ReleaseLedger {
    file: File,
    held: [u8; SLOTS],
}

impl ReleaseLedger {
    /// Never wait on another input owner: doing so can deadlock the AppKit thread.
    pub(crate) fn acquire(path: &Path) -> io::Result<Option<Self>> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(path)?;
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = file.metadata()?;
            if !meta.is_file()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o077 != 0
                || meta.nlink() != 1
            {
                return Err(io::Error::other("unsafe input recovery file"));
            }
        }
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(error)) => return Err(error),
        }
        let mut held = [0; SLOTS];
        match file.metadata()?.len() {
            0 => {
                file.write_all(&held)?;
                file.sync_data()?;
            }
            n if n == SLOTS as u64 => file.read_exact(&mut held)?,
            _ => return Err(io::Error::other("invalid input recovery file length")),
        }
        if held.iter().any(|&v| v > 1) {
            return Err(io::Error::other("invalid input recovery state"));
        }
        Ok(Some(Self { file, held }))
    }

    pub(crate) fn empty(&self) -> bool {
        !self.held.contains(&1)
    }

    pub(crate) fn contains(&self, slot: usize) -> bool {
        self.held.get(slot) == Some(&1)
    }

    fn set(&mut self, slot: usize, down: bool) -> io::Result<()> {
        if slot >= SLOTS {
            return Err(io::Error::other("invalid input recovery slot"));
        }
        self.file.seek(SeekFrom::Start(slot as u64))?;
        self.file.write_all(&[u8::from(down)])?;
        self.file.sync_data()?;
        self.held[slot] = u8::from(down);
        Ok(())
    }

    /// Persist before sending. Even a failed press can have reached the OS.
    pub(crate) fn press(
        &mut self,
        slot: usize,
        send: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        self.set(slot, true)
            .map_err(|e| format!("cannot record input release: {e}"))?;
        send()
    }

    /// Clear only after the release API succeeds. Failed releases remain retryable.
    pub(crate) fn release(
        &mut self,
        slot: usize,
        send: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if !self.contains(slot) {
            return Ok(());
        }
        send()?;
        self.set(slot, false)
            .map_err(|e| format!("cannot clear input release: {e}"))
    }

    pub(crate) fn recover(
        &mut self,
        mut release: impl FnMut(usize) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut error = None;
        for slot in (0..SLOTS).rev() {
            if let Err(e) = self.release(slot, || release(slot)) {
                error = Some(e);
            }
        }
        error.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "pab-release-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> std::path::PathBuf {
            self.0.join("held")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn failed_press_is_recorded_and_failed_release_is_retained() {
        let f = Fixture::new();
        let mut ledger = ReleaseLedger::acquire(&f.path()).unwrap().unwrap();
        assert!(ledger.press(55, || Err("partial press".into())).is_err());
        assert!(ledger.release(55, || Err("denied".into())).is_err());
        drop(ledger);
        let mut ledger = ReleaseLedger::acquire(&f.path()).unwrap().unwrap();
        assert!(ledger.contains(55));
        ledger
            .recover(|slot| {
                assert_eq!(slot, 55);
                Ok(())
            })
            .unwrap();
        assert!(ledger.empty());
    }
    #[test]
    fn live_owner_cannot_be_recovered_by_another_handle() {
        let f = Fixture::new();
        let _owner = ReleaseLedger::acquire(&f.path()).unwrap().unwrap();
        assert!(ReleaseLedger::acquire(&f.path()).unwrap().is_none());
    }
    #[test]
    fn recovery_continues_after_error_and_never_replays_presses() {
        let f = Fixture::new();
        let mut ledger = ReleaseLedger::acquire(&f.path()).unwrap().unwrap();
        for slot in [55, 0, 256] {
            ledger.press(slot, || Ok(())).unwrap();
        }
        let mut released = vec![];
        assert!(
            ledger
                .recover(|slot| {
                    released.push(slot);
                    if slot == 55 {
                        Err("denied".into())
                    } else {
                        Ok(())
                    }
                })
                .is_err()
        );
        assert_eq!(released, [256, 55, 0]);
        released.clear();
        ledger
            .recover(|slot| {
                released.push(slot);
                Ok(())
            })
            .unwrap();
        assert_eq!(released, [55]);
    }
    #[test]
    fn invalid_record_fails_closed() {
        let f = Fixture::new();
        std::fs::write(f.path(), b"broken").unwrap();
        assert!(ReleaseLedger::acquire(&f.path()).is_err());
        std::fs::write(f.path(), [2; SLOTS]).unwrap();
        assert!(ReleaseLedger::acquire(&f.path()).is_err());
    }
    #[test]
    fn record_failure_never_sends_input_and_unknown_release_is_ignored() {
        let f = Fixture::new();
        let mut ledger = ReleaseLedger::acquire(&f.path()).unwrap().unwrap();
        assert!(ledger.press(SLOTS, || panic!("must not send")).is_err());
        ledger.release(55, || panic!("not our key")).unwrap();
    }
    #[test]
    fn successful_release_is_not_recovered_twice() {
        let f = Fixture::new();
        let mut ledger = ReleaseLedger::acquire(&f.path()).unwrap().unwrap();
        ledger.press(55, || Ok(())).unwrap();
        ledger.release(55, || Ok(())).unwrap();
        drop(ledger);
        ReleaseLedger::acquire(&f.path())
            .unwrap()
            .unwrap()
            .recover(|_| panic!("already released"))
            .unwrap();
    }
    #[test]
    fn filesystem_write_error_prevents_press() {
        let f = Fixture::new();
        std::fs::write(f.path(), [0; SLOTS]).unwrap();
        let mut ledger = ReleaseLedger {
            file: File::open(f.path()).unwrap(),
            held: [0; SLOTS],
        };
        assert!(
            ledger
                .press(55, || panic!(
                    "must not send before successful journal write"
                ))
                .is_err()
        );
    }
    // This child records simulated presses only. No OS input events are injected.
    #[test]
    fn crash_child() {
        let Some(path) = std::env::var_os("PAB_RELEASE_TEST_PATH") else {
            return;
        };
        let mut ledger = ReleaseLedger::acquire(Path::new(&path)).unwrap().unwrap();
        ledger.press(55, || Ok(())).unwrap();
        ledger.press(257, || Ok(())).unwrap();
        println!("ready");
        io::stdout().flush().unwrap();
        loop {
            std::thread::park();
        }
    }
    #[test]
    fn killed_process_releases_lock_but_preserves_pending_releases() {
        use std::io::BufRead;
        let f = Fixture::new();
        let name = format!(
            "{}::crash_child",
            module_path!()
                .split_once("::")
                .map_or(module_path!(), |(_, rest)| rest)
        );
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env("PAB_RELEASE_TEST_PATH", f.path())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(
                reader.read_line(&mut line).unwrap(),
                0,
                "child exited before ready"
            );
            if line.trim() == "ready" {
                break;
            }
        }
        assert!(ReleaseLedger::acquire(&f.path()).unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
        let mut recovered = vec![];
        ReleaseLedger::acquire(&f.path())
            .unwrap()
            .unwrap()
            .recover(|slot| {
                recovered.push(slot);
                Ok(())
            })
            .unwrap();
        assert_eq!(recovered, [257, 55]);
    }
}
