//! Shared ZIP allocation, path and output budgets used by native platform adapters.
use crate::FileError;
use pab_protocol::FileOperationLimits;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, PathBuf},
};

pub const CENTRAL_BYTES: u64 = 2 * 1024 * 1024;
fn archive_io_error(phase: &str, error: std::io::Error) -> FileError {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => "not_found",
        std::io::ErrorKind::PermissionDenied => "access_denied",
        std::io::ErrorKind::AlreadyExists => "already_exists",
        _ => "io_error",
    };
    FileError::new(code, phase, error.to_string())
}

pub fn register_case_paths(
    paths: &mut BTreeMap<String, String>,
    name: &str,
) -> Result<(), FileError> {
    let mut prefix = String::new();
    for part in name.split('/') {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(part);
        let key = prefix.to_lowercase();
        if let Some(existing) = paths.get(&key) {
            if existing != &prefix {
                return Err(FileError::new(
                    "duplicate_archive_entry",
                    "validate_archive",
                    "ZIP directory components collide by case",
                ));
            }
        } else {
            paths.insert(key, prefix.clone());
        }
    }
    Ok(())
}

pub fn register(
    names: &mut BTreeMap<String, bool>,
    name: &str,
    directory: bool,
) -> Result<(), FileError> {
    let key = name.to_lowercase();
    if names.contains_key(&key)
        || names.iter().any(|(old, is_dir)| {
            (key.starts_with(&format!("{old}/")) && !is_dir)
                || (old.starts_with(&format!("{key}/")) && !directory)
        })
    {
        return Err(FileError::new(
            "duplicate_archive_entry",
            "validate_archive",
            "ZIP contains repeated/case-colliding paths or file/directory conflicts",
        ));
    }
    names.insert(key, directory);
    Ok(())
}

/// Bound central-directory allocation before asking the ZIP parser to allocate it.
pub fn footer(file: &mut File, limits: &FileOperationLimits) -> Result<usize, FileError> {
    let size = file
        .metadata()
        .map_err(|e| archive_io_error("stat_archive", e))?
        .len();
    if size < 22 || size > limits.max_bytes.saturating_add(CENTRAL_BYTES) {
        return Err(FileError::new(
            "archive_size_limit",
            "validate_archive",
            "ZIP file exceeds the compressed input budget or is too short",
        ));
    }
    let tail_size = size.min(65557);
    file.seek(SeekFrom::End(-(tail_size as i64)))
        .map_err(|e| archive_io_error("seek_archive", e))?;
    let mut tail = vec![0; tail_size as usize];
    file.read_exact(&mut tail)
        .map_err(|e| archive_io_error("read_archive", e))?;
    let eocd = (0..=tail.len() - 22)
        .rev()
        .find(|&i| {
            tail[i..i + 4] == *b"PK\x05\x06"
                && i + 22 + u16::from_le_bytes(tail[i + 20..i + 22].try_into().unwrap()) as usize
                    == tail.len()
        })
        .ok_or_else(|| {
            FileError::new(
                "invalid_archive",
                "validate_archive",
                "ZIP end record not found",
            )
        })?;
    let data = &tail[eocd..];
    let eocd_offset = size - tail_size + eocd as u64;
    if u16::from_le_bytes(data[4..6].try_into().unwrap()) != 0
        || u16::from_le_bytes(data[6..8].try_into().unwrap()) != 0
        || data[8..10] != data[10..12]
    {
        return Err(FileError::new(
            "unsupported_archive_format",
            "validate_archive",
            "multidisk ZIP is unsupported",
        ));
    }
    let mut count = u16::from_le_bytes(data[10..12].try_into().unwrap()) as u64;
    let mut central_size = u32::from_le_bytes(data[12..16].try_into().unwrap()) as u64;
    let mut central_offset = u32::from_le_bytes(data[16..20].try_into().unwrap()) as u64;
    if count == u16::MAX as u64
        || central_size == u32::MAX as u64
        || central_offset == u32::MAX as u64
    {
        if eocd_offset < 20 {
            return Err(FileError::new(
                "invalid_archive",
                "validate_archive",
                "missing ZIP64 locator",
            ));
        }
        file.seek(SeekFrom::Start(eocd_offset - 20))
            .map_err(|e| archive_io_error("seek_archive", e))?;
        let mut locator = [0; 20];
        file.read_exact(&mut locator)
            .map_err(|e| archive_io_error("read_archive", e))?;
        if locator[..4] != *b"PK\x06\x07"
            || locator[4..8] != [0; 4]
            || u32::from_le_bytes(locator[16..20].try_into().unwrap()) != 1
        {
            return Err(FileError::new(
                "unsupported_archive_format",
                "validate_archive",
                "invalid/multidisk ZIP64 locator",
            ));
        }
        let offset = u64::from_le_bytes(locator[8..16].try_into().unwrap());
        if offset.checked_add(56).is_none_or(|n| n > eocd_offset - 20) {
            return Err(FileError::new(
                "invalid_archive",
                "validate_archive",
                "ZIP64 record outside archive",
            ));
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| archive_io_error("seek_archive", e))?;
        let mut record = [0; 56];
        file.read_exact(&mut record)
            .map_err(|e| archive_io_error("read_archive", e))?;
        if record[..4] != *b"PK\x06\x06"
            || record[16..24] != [0; 8]
            || record[24..32] != record[32..40]
        {
            return Err(FileError::new(
                "unsupported_archive_format",
                "validate_archive",
                "invalid/multidisk ZIP64 record",
            ));
        }
        let record_len = u64::from_le_bytes(record[4..12].try_into().unwrap());
        if !(44..=CENTRAL_BYTES).contains(&record_len)
            || offset
                .checked_add(12 + record_len)
                .is_none_or(|n| n > eocd_offset - 20)
        {
            return Err(FileError::new(
                "invalid_archive",
                "validate_archive",
                "invalid ZIP64 record length",
            ));
        }
        count = u64::from_le_bytes(record[32..40].try_into().unwrap());
        central_size = u64::from_le_bytes(record[40..48].try_into().unwrap());
        central_offset = u64::from_le_bytes(record[48..56].try_into().unwrap());
    }
    if count > limits.max_entries as u64
        || central_size > CENTRAL_BYTES
        || central_offset
            .checked_add(central_size)
            .is_none_or(|n| n > eocd_offset)
    {
        return Err(FileError::new(
            "archive_metadata_limit",
            "validate_archive",
            "ZIP entries/central-directory size or range exceeds budget",
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|e| archive_io_error("seek_archive", e))?;
    Ok(count as usize)
}

/// Limit staging allocation even while compression/central directory is being written.
pub struct BoundedZipWriter {
    pub file: File,
    pub limit: u64,
}
impl Write for BoundedZipWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let pos = self.file.stream_position()?;
        if pos
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.limit)
        {
            return Err(std::io::Error::other("ZIP output byte budget exceeded"));
        }
        self.file.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
impl Seek for BoundedZipWriter {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

pub fn portable_relative(name: &str, max_depth: u32) -> Result<PathBuf, FileError> {
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
