use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub(super) struct RotatingFile {
    root: PathBuf,
    name: String,
    max_bytes: u64,
    max_files: usize,
    state: Mutex<FileState>,
}

struct FileState {
    file: Option<File>,
    bytes: u64,
}

pub(super) struct LogWriter(Arc<RotatingFile>);

impl LogWriter {
    pub(super) fn new(file: Arc<RotatingFile>) -> Self {
        Self(file)
    }
}

impl RotatingFile {
    pub(super) fn open(
        root: &Path,
        name: &str,
        max_bytes: u64,
        max_files: usize,
    ) -> io::Result<Self> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || max_bytes == 0
            || max_files == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid log rotation",
            ));
        }
        fs::create_dir_all(root)?;
        let path = root.join(format!("{name}.log"));
        let file = open_append(&path)?;
        let bytes = file.metadata()?.len();
        let writer = Self {
            root: root.to_owned(),
            name: name.to_owned(),
            max_bytes,
            max_files,
            state: Mutex::new(FileState {
                file: Some(file),
                bytes,
            }),
        };
        if bytes >= max_bytes {
            let mut state = writer
                .state
                .lock()
                .map_err(|_| io::Error::other("log lock poisoned"))?;
            writer.rotate(&mut state)?;
        }
        Ok(writer)
    }

    fn path(&self, index: usize) -> PathBuf {
        if index == 0 {
            self.root.join(format!("{}.log", self.name))
        } else {
            self.root.join(format!("{}.log.{index}", self.name))
        }
    }

    fn rotate(&self, state: &mut FileState) -> io::Result<()> {
        state.file.take();
        let oldest = self.path(self.max_files - 1);
        if oldest.exists() {
            fs::remove_file(&oldest)?;
        }
        for index in (0..self.max_files - 1).rev() {
            let previous = self.path(index);
            if previous.exists() {
                fs::rename(previous, self.path(index + 1))?;
            }
        }
        state.file = Some(open_append(&self.path(0))?);
        state.bytes = 0;
        Ok(())
    }
}

impl Write for &RotatingFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("log lock poisoned"))?;
        let mut remaining = bytes;
        while !remaining.is_empty() {
            if state.bytes >= self.max_bytes {
                self.rotate(&mut state)?;
            }
            let available = (self.max_bytes - state.bytes) as usize;
            let length = remaining.len().min(available);
            if let Some(file) = state.file.as_mut() {
                file.write_all(&remaining[..length])?;
                state.bytes += length as u64;
            }
            remaining = &remaining[length..];
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("log lock poisoned"))?;
        match state.file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        (&*self.0).write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self.0).flush()
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_at_the_size_limit_and_keeps_five_files() {
        let directory = tempfile::tempdir().unwrap();
        let logger = RotatingFile::open(directory.path(), "executor", 16, 5).unwrap();
        for index in 0..10 {
            let line = format!("entry-{index:02}\n");
            (&logger).write_all(line.as_bytes()).unwrap();
        }
        let files = fs::read_dir(directory.path()).unwrap().count();
        assert_eq!(files, 5);
        for index in 0..5 {
            assert!(logger.path(index).metadata().unwrap().len() <= 16);
        }
        assert!(
            fs::read_to_string(logger.path(0))
                .unwrap()
                .contains("entry-09")
        );
    }
}
