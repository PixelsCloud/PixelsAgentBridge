use super::{
    filesystem::FileError, filesystem_bulk::Work, filesystem_bulk_io as io, filesystem_io as base,
};
use pab_protocol::FileOperationLimits;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

const CENTRAL_BYTES: u64 = 2 * 1024 * 1024;
fn zip_error(phase: &str, e: impl std::fmt::Display) -> FileError {
    FileError::new("invalid_archive", phase, e.to_string())
}
struct ZipEntry {
    index: usize,
    relative: PathBuf,
    directory: bool,
    size: u64,
}

pub(super) fn create(
    work: &mut Work,
    sources: &[String],
    target: &Path,
    overwrite: bool,
    limits: &FileOperationLimits,
) -> Result<(), FileError> {
    io::inspect(target, true)?;
    let original = io::destination(target, overwrite)?;
    let _target = io::lock(work, target)?;
    let mut guards = Vec::new();
    let mut all = Vec::new();
    let mut names = BTreeMap::new();
    let mut case_paths = BTreeMap::new();
    let mut total = 0u64;
    let mut central_budget = 0usize;
    for (index, source) in sources.iter().enumerate() {
        work.check()?;
        let root = Path::new(source);
        io::inspect(root, false)?;
        io::separate(root, target)?;
        for earlier in &sources[..index] {
            io::separate(root, Path::new(earlier))?;
        }
        guards.push(io::lock(work, root)?);
        let base_name = root.file_name().and_then(|s| s.to_str()).ok_or_else(|| {
            FileError::new(
                "invalid_path",
                "planning",
                "ZIP sources must have a UTF-8 basename and cannot be roots",
            )
        })?;
        for entry in io::manifest(work, root, true, limits)? {
            if all.len() >= limits.max_entries as usize {
                return Err(FileError::new(
                    "entry_limit",
                    "planning",
                    "combined ZIP sources exceed max_entries",
                ));
            }
            let name = if entry.relative.as_os_str().is_empty() {
                base_name.to_owned()
            } else {
                format!(
                    "{base_name}/{}",
                    entry.relative.to_str().unwrap().replace('\\', "/")
                )
            };
            io::portable_relative(&name, limits.max_depth)?;
            register(&mut names, &name, entry.meta.is_dir())?;
            register_case_paths(&mut case_paths, &name)?;
            // Header + UTF-8 name + worst-case ZIP64/Unicode extra fields.
            central_budget += 256 + name.len() * 2;
            if central_budget > CENTRAL_BYTES as usize {
                return Err(FileError::new(
                    "archive_metadata_limit",
                    "planning",
                    "ZIP central directory would exceed its budget",
                ));
            }
            if entry.meta.is_file() {
                total = total.checked_add(entry.meta.len()).ok_or_else(|| {
                    FileError::new("bytes_limit", "planning", "combined size overflow")
                })?;
                if total > limits.max_bytes {
                    return Err(FileError::new(
                        "bytes_limit",
                        "planning",
                        "combined ZIP sources exceed max_bytes",
                    ));
                }
            }
            all.push((name, entry));
        }
    }
    work.reply.progress.as_mut().unwrap().total_bytes = total;
    work.reply.mutation.as_mut().unwrap().total_entries = all.len() as u32;
    work.phase("compressing")?;
    let mut staging = io::stage(target)?;
    let file = staging.file.take().unwrap();
    let mut writer = ZipWriter::new(BoundedZipWriter {
        file,
        limit: limits.max_bytes.saturating_add(CENTRAL_BYTES),
    });
    for (name, entry) in &all {
        work.check()?;
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(6))
            .large_file(entry.meta.len() >= u32::MAX as u64);
        if entry.meta.is_dir() {
            writer
                .add_directory(format!("{name}/"), options)
                .map_err(|e| zip_error("compress_directory", e))?;
        } else {
            let mut source = File::open(&entry.path).map_err(|e| io::error("open_source", e))?;
            if !io::same(
                &entry.meta,
                &source.metadata().map_err(|e| io::error("stat_source", e))?,
            ) {
                return Err(FileError::new(
                    "version_conflict",
                    "compress",
                    "ZIP source changed after planning",
                ));
            }
            writer
                .start_file(name, options)
                .map_err(|e| zip_error("compress_file", e))?;
            io::stream(work, &mut source, &mut writer, entry.meta.len())?;
            if !io::same(
                &entry.meta,
                &source.metadata().map_err(|e| io::error("stat_source", e))?,
            ) || !io::same(&entry.meta, &io::inspect(&entry.path, false)?.unwrap())
            {
                return Err(FileError::new(
                    "version_conflict",
                    "compress",
                    "ZIP source changed while being read",
                ));
            }
        }
        work.reply.mutation.as_mut().unwrap().processed_entries += 1;
        work.record(false)?;
    }
    let mut file = writer
        .finish()
        .map_err(|e| zip_error("finish_archive", e))?
        .file;
    let size = file
        .metadata()
        .map_err(|e| io::error("stat_archive", e))?
        .len();
    if size > limits.max_bytes.saturating_add(CENTRAL_BYTES) {
        return Err(FileError::new(
            "bytes_limit",
            "compress",
            "ZIP output exceeds its bounded overhead allowance",
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|e| io::error("seek_archive", e))?;
    let hash = io::hash_reader(work, &mut file, size)?.0;
    staging.file = Some(file);
    work.phase("publishing")?;
    io::publish(work, &mut staging, target, original.as_ref(), &hash, size)?;
    work.reply.metadata = Some(base::metadata(
        &fs::metadata(target).map_err(|e| io::error("stat_archive", e))?,
    ));
    work.reply.metadata.as_mut().unwrap().sha256 = Some(hash);
    Ok(())
}

pub(super) fn extract(
    work: &mut Work,
    source: &Path,
    target: &Path,
    overwrite: bool,
    max_ratio: u32,
    limits: &FileOperationLimits,
) -> Result<(), FileError> {
    let original = io::inspect(source, false)?.unwrap();
    if !original.is_file() {
        return Err(FileError::new(
            "not_regular_file",
            "archive",
            "ZIP source must be an ordinary file",
        ));
    }
    io::inspect(target, true)?;
    io::separate(source, target)?;
    if io::inspect(target, true)?.is_some_and(|m| !m.is_dir()) {
        return Err(FileError::new(
            "not_directory",
            "archive",
            "extraction destination must be a directory",
        ));
    }
    let _source = io::lock(work, source)?;
    let _target = io::lock(work, target)?;
    let mut file = File::open(source).map_err(|e| io::error("open_archive", e))?;
    if !io::same(
        &original,
        &file.metadata().map_err(|e| io::error("stat_archive", e))?,
    ) {
        return Err(FileError::new(
            "version_conflict",
            "archive",
            "ZIP changed before opening",
        ));
    }
    let expected_count = footer(&mut file, limits)?;
    let proof = file
        .try_clone()
        .map_err(|e| io::error("clone_archive", e))?;
    let mut archive = ZipArchive::new(file).map_err(|e| zip_error("parse_archive", e))?;
    if archive.len() != expected_count {
        return Err(FileError::new(
            "duplicate_archive_entry",
            "validate_archive",
            "central-directory count is inconsistent or entries are duplicated",
        ));
    }
    let mut names = BTreeMap::new();
    let mut case_paths = BTreeMap::new();
    let mut plan = Vec::new();
    let mut directories = BTreeSet::from([PathBuf::new()]);
    let mut total = 0u64;
    for index in 0..archive.len() {
        work.check()?;
        let entry = archive
            .by_index_raw(index)
            .map_err(|e| zip_error("validate_archive", e))?;
        let name = std::str::from_utf8(entry.name_raw()).map_err(|_| {
            FileError::new(
                "non_utf8_path",
                "validate_archive",
                "ZIP entry names must be UTF-8",
            )
        })?;
        let relative = io::portable_relative(name, limits.max_depth)?;
        register(&mut names, name.trim_end_matches('/'), entry.is_dir())?;
        register_case_paths(&mut case_paths, name.trim_end_matches('/'))?;
        let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
        if entry.is_symlink()
            || !matches!(mode, 0 | 0o100000 | 0o040000)
            || (mode == 0o040000 && !entry.is_dir())
            || (mode == 0o100000 && entry.is_dir())
        {
            return Err(FileError::new(
                "unsupported_archive_entry",
                "validate_archive",
                "ZIP links, device files and inconsistent entry types are unsupported",
            ));
        }
        if entry.encrypted()
            || !matches!(
                entry.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
        {
            return Err(FileError::new(
                "unsupported_archive_format",
                "validate_archive",
                "only unencrypted stored/deflated ZIP entries are supported",
            ));
        }
        if entry.is_dir() && entry.size() != 0 {
            return Err(FileError::new(
                "invalid_archive",
                "validate_archive",
                "directory entry contains file data",
            ));
        }
        if entry.size() > entry.compressed_size().saturating_mul(max_ratio as u64) {
            return Err(FileError::new(
                "compression_ratio_limit",
                "validate_archive",
                "ZIP entry exceeds max_ratio",
            ));
        }
        total = total.checked_add(entry.size()).ok_or_else(|| {
            FileError::new(
                "bytes_limit",
                "validate_archive",
                "uncompressed size overflow",
            )
        })?;
        if total > limits.max_bytes {
            return Err(FileError::new(
                "bytes_limit",
                "validate_archive",
                "ZIP uncompressed total exceeds max_bytes",
            ));
        }
        let dest = target.join(&relative);
        if entry.is_dir() {
            if io::inspect(&dest, true)?.is_some_and(|m| !m.is_dir()) {
                return Err(FileError::new(
                    "not_directory",
                    "validate_archive",
                    "entry conflicts with an existing file",
                ));
            }
        } else {
            io::destination(&dest, overwrite)?;
        }
        let mut parent = if entry.is_dir() {
            Some(relative.as_path())
        } else {
            relative.parent()
        };
        while let Some(path) = parent {
            directories.insert(path.to_path_buf());
            if directories.len() + plan.len() > limits.max_entries as usize {
                return Err(FileError::new(
                    "entry_limit",
                    "validate_archive",
                    "ZIP files plus implicit directories exceed max_entries",
                ));
            }
            parent = path.parent();
        }
        if !entry.is_dir() {
            plan.push(ZipEntry {
                index,
                relative,
                directory: false,
                size: entry.size(),
            });
        }
        if directories.len() + plan.len() > limits.max_entries as usize {
            return Err(FileError::new(
                "entry_limit",
                "validate_archive",
                "ZIP files plus implicit directories exceed max_entries",
            ));
        }
    }
    for relative in directories {
        let dest = target.join(&relative);
        if io::inspect(&dest, true)?.is_some_and(|m| !m.is_dir()) {
            return Err(FileError::new(
                "not_directory",
                "validate_archive",
                "implicit ZIP directory conflicts with existing file",
            ));
        }
        plan.push(ZipEntry {
            index: usize::MAX,
            relative,
            directory: true,
            size: 0,
        });
    }
    if !io::same(
        &original,
        &proof.metadata().map_err(|e| io::error("stat_archive", e))?,
    ) || !io::same(&original, &io::inspect(source, false)?.unwrap())
    {
        return Err(FileError::new(
            "version_conflict",
            "extract",
            "ZIP changed during preflight",
        ));
    }
    work.reply.progress.as_mut().unwrap().total_bytes = total;
    work.reply.mutation.as_mut().unwrap().total_entries = plan.len() as u32;
    work.phase("extracting")?;
    plan.sort_by(|a, b| {
        a.relative
            .components()
            .count()
            .cmp(&b.relative.components().count())
            .then(a.relative.cmp(&b.relative))
    });
    for entry in plan {
        work.check()?;
        let dest = target.join(&entry.relative);
        if entry.directory {
            io::mkdir(work, &dest)?;
        } else {
            io::mkdir(work, dest.parent().unwrap())?;
            let before = io::destination(&dest, overwrite)?;
            let mut staging = io::stage(&dest)?;
            let mut input = archive
                .by_index(entry.index)
                .map_err(|e| zip_error("open_entry", e))?;
            let hash = io::stream(work, &mut input, staging.file.as_mut().unwrap(), entry.size)?; // EOF validates CRC before publication.
            if !io::same(
                &original,
                &proof.metadata().map_err(|e| io::error("stat_archive", e))?,
            ) || !io::same(&original, &io::inspect(source, false)?.unwrap())
            {
                return Err(FileError::new(
                    "version_conflict",
                    "extract",
                    "ZIP changed during extraction",
                ));
            }
            io::publish(
                work,
                &mut staging,
                &dest,
                before.as_ref(),
                &hash,
                entry.size,
            )?;
        }
        work.reply.mutation.as_mut().unwrap().processed_entries += 1;
        work.record(true)?;
    }
    if !io::same(
        &original,
        &proof.metadata().map_err(|e| io::error("stat_archive", e))?,
    ) || !io::same(&original, &io::inspect(source, false)?.unwrap())
    {
        return Err(FileError::new(
            "version_conflict",
            "extract",
            "ZIP changed during extraction",
        ));
    }
    Ok(())
}

fn register_case_paths(paths: &mut BTreeMap<String, String>, name: &str) -> Result<(), FileError> {
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

fn register(
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
fn footer(file: &mut File, limits: &FileOperationLimits) -> Result<usize, FileError> {
    let size = file
        .metadata()
        .map_err(|e| io::error("stat_archive", e))?
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
        .map_err(|e| io::error("seek_archive", e))?;
    let mut tail = vec![0; tail_size as usize];
    file.read_exact(&mut tail)
        .map_err(|e| io::error("read_archive", e))?;
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
            .map_err(|e| io::error("seek_archive", e))?;
        let mut locator = [0; 20];
        file.read_exact(&mut locator)
            .map_err(|e| io::error("read_archive", e))?;
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
            .map_err(|e| io::error("seek_archive", e))?;
        let mut record = [0; 56];
        file.read_exact(&mut record)
            .map_err(|e| io::error("read_archive", e))?;
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
        .map_err(|e| io::error("seek_archive", e))?;
    Ok(count as usize)
}

/// Limit staging allocation even while compression/central directory is being written.
struct BoundedZipWriter {
    file: File,
    limit: u64,
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
