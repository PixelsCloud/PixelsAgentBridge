use super::{
    filesystem::FileError, filesystem_bulk::Work, filesystem_engine::FileGuard,
    filesystem_io as base,
};
use pab_protocol::FileOperationLimits;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};

pub(super) struct Entry {
    pub path: PathBuf,
    pub relative: PathBuf,
    pub meta: Metadata,
    pub hash: Option<String>,
}
pub(super) struct Stage {
    pub path: PathBuf,
    pub file: Option<File>,
}
impl Drop for Stage {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn error(phase: &str, e: std::io::Error) -> FileError {
    base::io_error(phase, e)
}
pub(super) fn inspect(path: &Path, allow_missing: bool) -> Result<Option<Metadata>, FileError> {
    base::validate_path(path)?;
    if path.to_str().is_none_or(|s| s.len() > 4096) {
        return Err(FileError::new(
            "invalid_path",
            "validate",
            "path must be UTF-8 and at most 4096 bytes",
        ));
    }
    let mut current = PathBuf::new();
    let mut final_meta = None;
    for part in path.components() {
        current.push(part);
        let meta = match fs::symlink_metadata(&current) {
            Ok(meta) => meta,
            Err(e) if allow_missing && e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(error("inspect_path", e)),
        };
        #[cfg(target_os = "macos")]
        let meta = base::macos_system_alias(&current, &meta).unwrap_or(meta);
        if base::is_link(&meta) {
            return Err(FileError::new(
                "link_not_supported",
                "inspect_path",
                "bulk operations do not follow links or reparse points",
            ));
        }
        if current != path && !meta.is_dir() {
            return Err(FileError::new(
                "not_directory",
                "inspect_path",
                "a parent component is not a directory",
            ));
        }
        final_meta = Some(meta);
    }
    Ok(final_meta)
}
pub(super) fn same_identity(a: &Metadata, b: &Metadata) -> bool {
    let common = a.is_file() == b.is_file() && a.is_dir() == b.is_dir();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        common && a.dev() == b.dev() && a.ino() == b.ino()
    }
    #[cfg(not(unix))]
    {
        common && a.created().ok() == b.created().ok()
    }
}
pub(super) fn same(a: &Metadata, b: &Metadata) -> bool {
    let common = a.is_file() == b.is_file()
        && a.is_dir() == b.is_dir()
        && a.len() == b.len()
        && a.modified().ok() == b.modified().ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        common && a.dev() == b.dev() && a.ino() == b.ino()
    }
    #[cfg(not(unix))]
    {
        common && a.created().ok() == b.created().ok()
    }
}
pub(super) fn normalized(path: &Path) -> Result<PathBuf, FileError> {
    let value = if path.exists() {
        fs::canonicalize(path).map_err(|e| error("canonicalize", e))?
    } else {
        let mut ancestor = path;
        let mut missing = Vec::new();
        while !ancestor.exists() {
            missing.push(
                ancestor
                    .file_name()
                    .ok_or_else(|| {
                        FileError::new("invalid_path", "validate", "path has no existing ancestor")
                    })?
                    .to_owned(),
            );
            ancestor = ancestor
                .parent()
                .ok_or_else(|| FileError::new("invalid_path", "validate", "path has no parent"))?;
        }
        let mut value = fs::canonicalize(ancestor).map_err(|e| error("canonicalize", e))?;
        for part in missing.into_iter().rev() {
            value.push(part);
        }
        value
    };
    #[cfg(windows)]
    {
        Ok(PathBuf::from(value.to_string_lossy().to_lowercase()))
    }
    #[cfg(not(windows))]
    {
        Ok(value)
    }
}
pub(super) fn separate(source: &Path, target: &Path) -> Result<(), FileError> {
    let source = normalized(source)?;
    let target = normalized(target)?;
    if source.starts_with(&target) || target.starts_with(&source) {
        return Err(FileError::new(
            "overlapping_paths",
            "validate",
            "source and destination must not be equal or contain one another",
        ));
    }
    Ok(())
}
pub(super) fn lock(work: &Work, path: &Path) -> Result<FileGuard, FileError> {
    work.handle
        .block_on(work.service.upload_locks.try_acquire(path))
        .map_err(|e| error("lock", e))?
        .ok_or_else(|| {
            FileError::new(
                "path_busy",
                "lock",
                "another PAB operation holds this path or an ancestor/descendant",
            )
        })
}
pub(super) fn not_root(path: &Path) -> Result<(), FileError> {
    if path.file_name().is_none() || path.parent().is_none() {
        Err(FileError::new(
            "root_not_supported",
            "validate",
            "root or volume root cannot be moved or deleted",
        ))
    } else {
        Ok(())
    }
}
pub(super) fn manifest(
    work: &Work,
    root: &Path,
    recursive: bool,
    limits: &FileOperationLimits,
) -> Result<Vec<Entry>, FileError> {
    inspect(root, false)?;
    let mut pending = vec![(root.to_path_buf(), 0u32)];
    let mut entries = Vec::new();
    let mut size = 0u64;
    let mut path_bytes = 0usize;
    while let Some((path, depth)) = pending.pop() {
        work.check()?;
        let meta = inspect(&path, false)?.unwrap();
        if !meta.is_file() && !meta.is_dir() {
            return Err(FileError::new(
                "not_regular_file",
                "planning",
                "only ordinary files and directories are supported",
            ));
        }
        if depth > limits.max_depth {
            return Err(FileError::new(
                "depth_limit",
                "planning",
                "tree exceeds max_depth",
            ));
        }
        if meta.is_file() {
            size = size
                .checked_add(meta.len())
                .ok_or_else(|| FileError::new("bytes_limit", "planning", "file sizes overflow"))?;
            if size > limits.max_bytes {
                return Err(FileError::new(
                    "bytes_limit",
                    "planning",
                    "tree exceeds max_bytes",
                ));
            }
        }
        path_bytes += path.to_str().unwrap().len();
        if path_bytes > 2 * 1024 * 1024 {
            return Err(FileError::new(
                "path_bytes_limit",
                "planning",
                "manifest path budget exceeded",
            ));
        }
        if meta.is_dir() {
            for item in fs::read_dir(&path).map_err(|e| error("list_directory", e))? {
                let child = item.map_err(|e| error("list_directory", e))?.path();
                if !recursive {
                    return Err(FileError::new(
                        "recursive_required",
                        "planning",
                        "nonempty directory requires recursive=true",
                    ));
                }
                if entries.len() + pending.len() + 1 >= limits.max_entries as usize {
                    return Err(FileError::new(
                        "entry_limit",
                        "planning",
                        "tree exceeds max_entries",
                    ));
                }
                pending.push((child, depth + 1));
            }
        }
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        entries.push(Entry {
            path,
            relative,
            meta,
            hash: None,
        });
    }
    entries.sort_by(|a, b| {
        a.relative
            .components()
            .count()
            .cmp(&b.relative.components().count())
            .then(a.relative.cmp(&b.relative))
    });
    Ok(entries)
}
pub(super) fn destination(path: &Path, overwrite: bool) -> Result<Option<Metadata>, FileError> {
    let value = inspect(path, true)?;
    if let Some(value) = &value {
        if !value.is_file() {
            return Err(FileError::new(
                "not_regular_file",
                "destination",
                "file destination is not a regular file",
            ));
        }
        if !overwrite {
            return Err(FileError::new(
                "already_exists",
                "destination",
                "destination exists; overwrite must be explicit",
            ));
        }
        if value.permissions().readonly() {
            return Err(FileError::new(
                "access_denied",
                "destination",
                "destination is readonly",
            ));
        }
    }
    Ok(value)
}
pub(super) fn stage(path: &Path) -> Result<Stage, FileError> {
    let parent = path
        .parent()
        .ok_or_else(|| FileError::new("invalid_path", "stage", "missing parent"))?;
    if !inspect(parent, false)?.is_some_and(|m| m.is_dir()) {
        return Err(FileError::new(
            "missing_parent",
            "stage",
            "destination parent must exist",
        ));
    }
    let temp = parent.join(format!(".pab-file-{}.tmp", pab_protocol::RequestId::new()));
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&temp)
        .map_err(|e| error("create_staging", e))?;
    Ok(Stage {
        path: temp,
        file: Some(file),
    })
}
pub(super) fn hash_reader(
    work: &Work,
    reader: &mut impl Read,
    max_bytes: u64,
) -> Result<(String, u64), FileError> {
    let mut hasher = Sha256::new();
    let mut count = 0u64;
    let mut buffer = vec![0; 64 * 1024];
    loop {
        work.check()?;
        let n = reader
            .read(&mut buffer)
            .map_err(|e| error("verify_read", e))?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > max_bytes {
            return Err(FileError::new(
                "bytes_limit",
                "verify",
                "verification exceeded its byte budget",
            ));
        }
        hasher.update(&buffer[..n]);
    }
    Ok((hex::encode(hasher.finalize()), count))
}
pub(super) fn stream(
    work: &mut Work,
    reader: &mut impl Read,
    writer: &mut impl Write,
    expected_size: u64,
) -> Result<String, FileError> {
    let mut hasher = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = vec![0; 64 * 1024];
    loop {
        work.check()?;
        let n = reader
            .read(&mut buffer)
            .map_err(|e| error("read_source", e))?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > expected_size {
            return Err(FileError::new(
                "version_conflict",
                "read_source",
                "source exceeded the planned size",
            ));
        }
        writer
            .write_all(&buffer[..n])
            .map_err(|e| error("write_staging", e))?;
        hasher.update(&buffer[..n]);
        work.reply.progress.as_mut().unwrap().completed_bytes += n as u64;
        work.record(false)?;
    }
    if bytes != expected_size {
        return Err(FileError::new(
            "version_conflict",
            "read_source",
            "source length differs from its planned size",
        ));
    }
    Ok(hex::encode(hasher.finalize()))
}
pub(super) fn publish(
    work: &mut Work,
    staging: &mut Stage,
    path: &Path,
    original: Option<&Metadata>,
    hash: &str,
    size: u64,
) -> Result<(), FileError> {
    let mut file = staging.file.take().unwrap();
    file.sync_all().map_err(|e| error("sync_staging", e))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| error("seek_staging", e))?;
    let (actual, count) = hash_reader(work, &mut file, size)?;
    if actual != hash || count != size {
        return Err(FileError::new(
            "hash_mismatch",
            "verify_staging",
            "staged file did not match the written data",
        ));
    }
    drop(file);
    work.gate("before_publish");
    work.check()?;
    let current = inspect(path, true)?;
    match (original, current.as_ref()) {
        (Some(old), Some(new)) if same(old, new) => {}
        (None, None) => {}
        _ => {
            return Err(FileError::new(
                "version_conflict",
                "publish",
                "destination changed before publication",
            ));
        }
    }
    if original.is_some() {
        fs::rename(&staging.path, path).map_err(|e| error("publish", e))?;
    } else {
        fs::hard_link(&staging.path, path).map_err(|e| error("publish_exclusive", e))?;
    }
    work.effect(path, "published", Some(hash.to_owned()))
}
pub(super) fn mkdir(work: &mut Work, path: &Path) -> Result<(), FileError> {
    work.check()?;
    if let Some(value) = inspect(path, true)? {
        if value.is_dir() {
            return Ok(());
        }
        return Err(FileError::new(
            "not_directory",
            "mkdir",
            "destination component is not a directory",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| FileError::new("invalid_path", "mkdir", "missing parent"))?;
    if inspect(parent, true)?.is_none() {
        mkdir(work, parent)?;
    }
    inspect(parent, false)?;
    work.check()?;
    match fs::create_dir(path) {
        Ok(()) => work.effect(path, "mkdir", None),
        Err(e)
            if e.kind() == std::io::ErrorKind::AlreadyExists
                && inspect(path, false)?.is_some_and(|m| m.is_dir()) =>
        {
            Ok(())
        }
        Err(e) => Err(error("mkdir", e)),
    }
}

pub(super) fn copy_or_move(
    work: &mut Work,
    source: &Path,
    target: &Path,
    recursive: bool,
    overwrite: bool,
    moving: bool,
    limits: &FileOperationLimits,
) -> Result<(), FileError> {
    inspect(source, false)?;
    inspect(target, true)?;
    separate(source, target)?;
    if moving {
        not_root(source)?;
    }
    let _source_lock = lock(work, source)?;
    let _target_lock = lock(work, target)?;
    let mut entries = manifest(work, source, recursive, limits)?;
    let source_is_dir = entries[0].meta.is_dir();
    if inspect(target, true)?.is_some() && !overwrite {
        return Err(FileError::new(
            "already_exists",
            "planning",
            "destination exists; overwrite must be explicit",
        ));
    }
    for entry in &entries {
        let dest = if entry.relative.as_os_str().is_empty() {
            target.to_path_buf()
        } else {
            target.join(&entry.relative)
        };
        if entry.meta.is_file() {
            destination(&dest, overwrite)?;
        } else if inspect(&dest, true)?.is_some_and(|m| !m.is_dir()) {
            return Err(FileError::new(
                "not_directory",
                "planning",
                "directory destination has an incompatible type",
            ));
        }
    }
    work.reply.progress.as_mut().unwrap().total_bytes = entries
        .iter()
        .filter(|e| e.meta.is_file())
        .map(|e| e.meta.len())
        .sum();
    work.reply.mutation.as_mut().unwrap().total_entries =
        entries.len() as u32 * if moving { 2 } else { 1 };
    work.phase("copying")?;
    for entry in &mut entries {
        work.check()?;
        let dest = if entry.relative.as_os_str().is_empty() {
            target.to_path_buf()
        } else {
            target.join(&entry.relative)
        };
        if entry.meta.is_dir() {
            mkdir(work, &dest)?;
        } else {
            let original = destination(&dest, overwrite)?;
            let mut input = File::open(&entry.path).map_err(|e| error("open_source", e))?;
            if !same(
                &entry.meta,
                &input.metadata().map_err(|e| error("stat_source", e))?,
            ) {
                return Err(FileError::new(
                    "version_conflict",
                    "copy",
                    "source changed after planning",
                ));
            }
            let mut staging = stage(&dest)?;
            let hash = stream(
                work,
                &mut input,
                staging.file.as_mut().unwrap(),
                entry.meta.len(),
            )?;
            if !same(
                &entry.meta,
                &input.metadata().map_err(|e| error("stat_source", e))?,
            ) || !same(&entry.meta, &inspect(&entry.path, false)?.unwrap())
            {
                return Err(FileError::new(
                    "version_conflict",
                    "copy",
                    "source changed while being copied",
                ));
            }
            drop(input);
            publish(
                work,
                &mut staging,
                &dest,
                original.as_ref(),
                &hash,
                entry.meta.len(),
            )?;
            entry.hash = Some(hash);
        }
        work.reply.mutation.as_mut().unwrap().processed_entries += 1;
        work.record(true)?;
    }
    if !source_is_dir {
        work.reply.metadata = Some(base::metadata(
            &fs::metadata(target).map_err(|e| error("stat_destination", e))?,
        ));
        work.reply.metadata.as_mut().unwrap().sha256 = entries[0].hash.clone();
    }
    if moving {
        work.gate("before_source_delete");
        work.check()?;
        work.phase("verifying_move")?;
        let current = manifest(work, source, recursive, limits)?;
        if current.len() != entries.len()
            || current.iter().zip(&entries).any(|(a, b)| {
                a.relative != b.relative
                    || !same_identity(&a.meta, &b.meta)
                    || (!b.meta.is_dir() && !same(&a.meta, &b.meta))
            })
        {
            return Err(FileError::new(
                "version_conflict",
                "verify_move",
                "source tree changed before removal",
            ));
        }
        for entry in entries.iter().filter(|e| e.meta.is_file()) {
            let dest = if entry.relative.as_os_str().is_empty() {
                target.to_path_buf()
            } else {
                target.join(&entry.relative)
            };
            inspect(&dest, false)?;
            let mut input = File::open(&dest).map_err(|e| error("verify_destination", e))?;
            if hash_reader(work, &mut input, entry.meta.len())?.0 != *entry.hash.as_ref().unwrap() {
                return Err(FileError::new(
                    "hash_mismatch",
                    "verify_move",
                    "destination no longer matches; source was not removed",
                ));
            }
        }
        work.phase("removing_source")?;
        remove_entries(work, &entries, false)?;
        work.reply.mutation.as_mut().unwrap().source_removed = true;
    }
    Ok(())
}
pub(super) fn delete(
    work: &mut Work,
    path: &Path,
    recursive: bool,
    limits: &FileOperationLimits,
) -> Result<(), FileError> {
    not_root(path)?;
    inspect(path, false)?;
    let _guard = lock(work, path)?;
    let entries = manifest(work, path, recursive, limits)?;
    work.reply.mutation.as_mut().unwrap().total_entries = entries.len() as u32;
    work.reply.progress.as_mut().unwrap().total_bytes = entries
        .iter()
        .filter(|e| e.meta.is_file())
        .map(|e| e.meta.len())
        .sum();
    work.phase("deleting")?;
    remove_entries(work, &entries, true)?;
    work.reply.mutation.as_mut().unwrap().source_removed = true;
    Ok(())
}
fn remove_entries(work: &mut Work, entries: &[Entry], count_bytes: bool) -> Result<(), FileError> {
    for entry in entries.iter().rev() {
        work.gate("before_delete");
        work.check()?;
        let current = inspect(&entry.path, false)?.unwrap();
        if (entry.meta.is_file() && !same(&entry.meta, &current))
            || (entry.meta.is_dir() && !same_identity(&entry.meta, &current))
        {
            return Err(FileError::new(
                "version_conflict",
                "delete",
                "planned source entry changed before deletion",
            ));
        }
        if let Some(expected) = &entry.hash {
            let target = Path::new(work.reply.destination.as_deref().unwrap());
            let dest = if entry.relative.as_os_str().is_empty() {
                target.to_path_buf()
            } else {
                target.join(&entry.relative)
            };
            for candidate in [&entry.path, &dest] {
                let meta = inspect(candidate, false)?.unwrap();
                if !meta.is_file() {
                    return Err(FileError::new(
                        "version_conflict",
                        "verify_move",
                        "move endpoint changed type",
                    ));
                }
                let mut file = File::open(candidate).map_err(|e| error("verify_move", e))?;
                if hash_reader(work, &mut file, entry.meta.len())?.0 != *expected {
                    return Err(FileError::new(
                        "hash_mismatch",
                        "verify_move",
                        "move endpoint changed before source removal",
                    ));
                }
            }
        }
        if entry.meta.is_file() {
            fs::remove_file(&entry.path).map_err(|e| error("delete_file", e))?;
            if count_bytes {
                work.reply.progress.as_mut().unwrap().completed_bytes += entry.meta.len();
            }
        } else {
            fs::remove_dir(&entry.path).map_err(|e| error("delete_directory", e))?;
        }
        work.reply.mutation.as_mut().unwrap().processed_entries += 1;
        work.effect(&entry.path, "deleted", None)?;
    }
    Ok(())
}

pub(super) fn portable_relative(name: &str, max_depth: u32) -> Result<PathBuf, FileError> {
    let name = name.strip_suffix('/').unwrap_or(name);
    if name.is_empty()
        || name.len() > 4096
        || name.starts_with('/')
        || name.contains(['\\', ':', '\0'])
    {
        return Err(FileError::new(
            "unsafe_archive_path",
            "validate_archive",
            "archive path is empty, absolute, ambiguous or contains a device/drive path",
        ));
    }
    let parts = name.split('/').collect::<Vec<_>>();
    if parts.len() > max_depth as usize {
        return Err(FileError::new(
            "depth_limit",
            "validate_archive",
            "archive path exceeds max_depth",
        ));
    }
    for part in parts {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part.is_empty()
            || part
                .chars()
                .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
            || part == "."
            || part == ".."
            || part.ends_with([' ', '.'])
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
        {
            return Err(FileError::new(
                "unsafe_archive_path",
                "validate_archive",
                "archive contains traversal or reserved/ambiguous names",
            ));
        }
    }
    let path = PathBuf::from(name);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(FileError::new(
            "unsafe_archive_path",
            "validate_archive",
            "archive contains an invalid path component",
        ));
    }
    Ok(path)
}
