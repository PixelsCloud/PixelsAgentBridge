use super::{
    filesystem::FileError, filesystem_bulk::Work, filesystem_bulk_io as io, filesystem_io as base,
};
use pab_protocol::FileOperationLimits;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Seek, SeekFrom},
    path::{Path, PathBuf},
};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use pab_file_codec::archive::{
    BoundedZipWriter, CENTRAL_BYTES, footer, register, register_case_paths,
};
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
