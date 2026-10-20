//! ZIP processing uses shared Desktop validation and bounded temporary files.
use super::{bulk::*, *};
use pab_file_codec::archive::{
    BoundedZipWriter, CENTRAL_BYTES, footer, portable_relative, register, register_case_paths,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

struct ZipEntry {
    index: usize,
    name: String,
    directory: bool,
    size: u64,
}

impl FileService {
    pub(super) async fn archive_create(
        &self,
        owner: &str,
        request: &FileSystemRequest,
        reply: &mut FileSystemReply,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(), FileError> {
        let FileSystemAction::ArchiveCreate {
            sources,
            overwrite,
            limits,
        } = &request.operation
        else {
            unreachable!()
        };
        let overwrite = *overwrite;
        let target = normalized(&request.path)?;
        protect_root(&target)?;
        let previous = self.inspect_destination(&target, false, overwrite).await?;
        let mut all = Vec::new();
        let mut names = BTreeMap::new();
        let mut case_paths = BTreeMap::new();
        let mut total = 0u64;
        let mut central = 0usize;
        for (index, source) in sources.iter().enumerate() {
            let source = normalized(source)?;
            protect_root(&source)?;
            separate(&source, &target)?;
            for earlier in &sources[..index] {
                separate(earlier, &source)?;
            }
            let base = source.rsplit('/').next().unwrap();
            for entry in self.inventory(&source, true, limits, cancel).await? {
                cancelled(cancel)?;
                if all.len() >= limits.max_entries as usize {
                    return Err(limit("entry_limit"));
                }
                let name = join(base, &entry.relative);
                portable_relative(&name, limits.max_depth)?;
                register(&mut names, &name, entry.value["directory"] == true)?;
                register_case_paths(&mut case_paths, &name)?;
                central += 256 + name.len() * 2;
                if central > CENTRAL_BYTES as usize {
                    return Err(limit("archive_metadata_limit"));
                }
                total = total
                    .checked_add(entry.value["size"].as_u64().unwrap_or(0))
                    .ok_or_else(|| limit("bytes_limit"))?;
                if total > limits.max_bytes {
                    return Err(limit("bytes_limit"));
                }
                all.push((name, entry));
            }
        }
        reply.mutation.as_mut().unwrap().total_entries = all.len() as u32;
        reply.progress.as_mut().unwrap().total_bytes = total;
        reply.mutation.as_mut().unwrap().phase = "compressing".into();
        self.bulk_progress(owner, reply).await?;
        let maximum = total.saturating_add(CENTRAL_BYTES).min(1024 * 1024 * 1024);
        self.io(json!({"action":"check_staging","path":"/","additional_bytes":maximum}))
            .await?;
        let stage = self.stage(Uuid::new_v4());
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .read(true)
            .open(&stage.0)
            .map_err(io_error)?;
        let mut writer = ZipWriter::new(BoundedZipWriter {
            file,
            limit: maximum,
        });
        for (name, entry) in all {
            cancelled(cancel)?;
            self.unchanged(&entry).await?;
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .compression_level(Some(6));
            if entry.value["directory"] == true {
                writer
                    .add_directory(format!("{name}/"), options)
                    .map_err(zip_error)?;
            } else {
                let (snapshot, value) = self
                    .snapshot(&entry.path, entry.value["size"].as_u64().unwrap())
                    .await?;
                self.unchanged(&entry).await?;
                let path = snapshot.0.clone();
                let stop = cancel.clone();
                let size = value["size"]
                    .as_u64()
                    .ok_or_else(|| limit("invalid_snapshot"))?;
                let hash = value["sha256"]
                    .as_str()
                    .ok_or_else(|| limit("invalid_snapshot"))?
                    .to_owned();
                writer = tokio::task::spawn_blocking(move || {
                    writer.start_file(name, options).map_err(zip_error)?;
                    let mut source = File::open(path).map_err(io_error)?;
                    let actual = copy_bounded(&mut source, &mut writer, size, &stop)?;
                    check_hash(Some(&hash), &actual)?;
                    Ok::<_, FileError>(writer)
                })
                .await
                .map_err(|_| limit("archive_worker_unconfirmed"))??;
            }
            reply.mutation.as_mut().unwrap().processed_entries += 1;
            reply.progress.as_mut().unwrap().completed_bytes +=
                entry.value["size"].as_u64().unwrap_or(0);
            self.bulk_progress(owner, reply).await?;
        }
        let hash = tokio::task::spawn_blocking(move || {
            let mut file = writer.finish().map_err(zip_error)?.file;
            file.sync_all().map_err(io_error)?;
            use std::io::{Seek, SeekFrom};
            file.seek(SeekFrom::Start(0)).map_err(io_error)?;
            let mut hash = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let size = file.read(&mut buffer).map_err(io_error)?;
                if size == 0 {
                    break;
                }
                hash.update(&buffer[..size]);
            }
            Ok::<_, FileError>(format!("{:x}", hash.finalize()))
        })
        .await
        .map_err(|_| limit("archive_worker_unconfirmed"))??;
        cancelled(cancel)?;
        if let Some(previous) = &previous {
            self.unchanged(&PlannedEntry {
                path: target.clone(),
                relative: String::new(),
                value: previous.clone(),
            })
            .await?;
        }
        reply.mutation.as_mut().unwrap().phase = "publishing".into();
        self.bulk_progress(owner, reply).await?;
        let mut meta = self
            .publish_stage(
                &target,
                &stage,
                overwrite,
                previous.as_ref().and_then(|value| value["sha256"].as_str()),
            )
            .await?;
        meta.sha256 = Some(hash.clone());
        reply.metadata = Some(meta);
        item(reply, &target, "archive_create", Some(hash), true, false);
        Ok(())
    }

    pub(super) async fn archive_extract(
        &self,
        owner: &str,
        request: &FileSystemRequest,
        reply: &mut FileSystemReply,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(), FileError> {
        let FileSystemAction::ArchiveExtract {
            destination,
            overwrite,
            max_ratio,
            limits,
        } = &request.operation
        else {
            unreachable!()
        };
        let (overwrite, max_ratio) = (*overwrite, *max_ratio);
        let source = normalized(&request.path)?;
        let target = normalized(destination)?;
        protect_root(&target)?;
        separate(&source, &target)?;
        let (snapshot, _) = self
            .snapshot(
                &source,
                limits
                    .max_bytes
                    .saturating_add(CENTRAL_BYTES)
                    .min(1024 * 1024 * 1024),
            )
            .await?;
        let path = snapshot.0.clone();
        let budget = limits.clone();
        let (mut archive, entries) =
            tokio::task::spawn_blocking(move || plan(&path, &budget, max_ratio))
                .await
                .map_err(|_| limit("archive_worker_unconfirmed"))??;
        let mut previous = Vec::new();
        for entry in &entries {
            cancelled(cancel)?;
            previous.push(
                self.inspect_destination(&join(&target, &entry.name), entry.directory, overwrite)
                    .await?,
            );
        }
        reply.mutation.as_mut().unwrap().total_entries = entries.len() as u32;
        reply.progress.as_mut().unwrap().total_bytes = entries.iter().map(|entry| entry.size).sum();
        reply.mutation.as_mut().unwrap().phase = "extracting".into();
        self.bulk_progress(owner, reply).await?;
        for (entry, previous) in entries.into_iter().zip(previous) {
            cancelled(cancel)?;
            let path = join(&target, &entry.name);
            if entry.directory {
                if previous.is_none() {
                    self.io(json!({"action":"mkdir","path":path,"parents":false,"exist_ok":false}))
                        .await?;
                    item(reply, &path, "mkdir", None, true, false);
                }
            } else {
                self.io(json!({"action":"check_staging","path":"/","additional_bytes":entry.size}))
                    .await?;
                let output = self.stage(Uuid::new_v4());
                let staged = output.0.clone();
                let stop = cancel.clone();
                let (next, hash) = tokio::task::spawn_blocking(move || {
                    let mut input = archive.by_index(entry.index).map_err(zip_error)?;
                    let mut file = std::fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(staged)
                        .map_err(io_error)?;
                    let hash = copy_bounded(&mut input, &mut file, entry.size, &stop)?;
                    file.sync_all().map_err(io_error)?;
                    drop(input);
                    Ok::<_, FileError>((archive, hash))
                })
                .await
                .map_err(|_| limit("archive_worker_unconfirmed"))??;
                archive = next;
                cancelled(cancel)?;
                if let Some(previous) = &previous {
                    self.unchanged(&PlannedEntry {
                        path: path.clone(),
                        relative: String::new(),
                        value: previous.clone(),
                    })
                    .await?;
                }
                self.publish_stage(
                    &path,
                    &output,
                    overwrite,
                    previous.as_ref().and_then(|value| value["sha256"].as_str()),
                )
                .await?;
                item(reply, &path, "archive_extract", Some(hash), true, false);
            }
            reply.mutation.as_mut().unwrap().processed_entries += 1;
            reply.progress.as_mut().unwrap().completed_bytes += entry.size;
            self.bulk_progress(owner, reply).await?;
        }
        Ok(())
    }
}

fn plan(
    path: &Path,
    limits: &FileOperationLimits,
    max_ratio: u32,
) -> Result<(ZipArchive<File>, Vec<ZipEntry>), FileError> {
    let mut file = File::open(path).map_err(io_error)?;
    let count = footer(&mut file, limits)?;
    let mut archive = ZipArchive::new(file).map_err(zip_error)?;
    if archive.len() != count {
        return Err(limit("duplicate_archive_entry"));
    }
    let mut names = BTreeMap::new();
    let mut cases = BTreeMap::new();
    let mut directories = BTreeSet::from([String::new()]);
    let mut files = Vec::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(zip_error)?;
        let name = std::str::from_utf8(entry.name_raw()).map_err(|_| limit("non_utf8_path"))?;
        portable_relative(name, limits.max_depth)?;
        let name = name.trim_end_matches('/').to_owned();
        register(&mut names, &name, entry.is_dir())?;
        register_case_paths(&mut cases, &name)?;
        let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
        if entry.is_symlink()
            || !matches!(mode, 0 | 0o100000 | 0o040000)
            || (mode == 0o040000 && !entry.is_dir())
            || (mode == 0o100000 && entry.is_dir())
        {
            return Err(limit("unsupported_archive_entry"));
        }
        if entry.encrypted()
            || !matches!(
                entry.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
        {
            return Err(limit("unsupported_archive_format"));
        }
        if entry.is_dir() && entry.size() != 0 {
            return Err(limit("invalid_archive"));
        }
        if entry.size() > entry.compressed_size().saturating_mul(u64::from(max_ratio)) {
            return Err(limit("compression_ratio_limit"));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| limit("bytes_limit"))?;
        if total > limits.max_bytes || entry.size() > 1024 * 1024 * 1024 {
            return Err(limit("bytes_limit"));
        }
        let mut parent = if entry.is_dir() {
            Some(name.as_str())
        } else {
            name.rsplit_once('/').map(|(parent, _)| parent)
        };
        while let Some(part) = parent {
            directories.insert(part.to_owned());
            parent = part.rsplit_once('/').map(|(parent, _)| parent);
        }
        if !entry.is_dir() {
            files.push(ZipEntry {
                index,
                name,
                directory: false,
                size: entry.size(),
            });
        }
        if directories.len() + files.len() > limits.max_entries as usize {
            return Err(limit("entry_limit"));
        }
    }
    files.extend(directories.into_iter().map(|name| ZipEntry {
        index: usize::MAX,
        name,
        directory: true,
        size: 0,
    }));
    files.sort_by(|a, b| {
        a.name
            .matches('/')
            .count()
            .cmp(&b.name.matches('/').count())
            .then(a.name.cmp(&b.name))
    });
    Ok((archive, files))
}

fn copy_bounded(
    source: &mut impl Read,
    destination: &mut impl Write,
    size: u64,
    cancel: &AtomicBool,
) -> Result<String, FileError> {
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        cancelled(cancel)?;
        let read = source.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| limit("bytes_limit"))?;
        if total > size {
            return Err(limit("bytes_limit"));
        }
        destination.write_all(&buffer[..read]).map_err(io_error)?;
        hash.update(&buffer[..read]);
    }
    if total != size {
        return Err(limit("version_conflict"));
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn zip_error(error: impl std::fmt::Display) -> FileError {
    FileError::new("invalid_archive", "archive", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = ZipWriter::new(File::create(path).unwrap());
        for (name, bytes) in entries {
            writer
                .start_file(
                    *name,
                    SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn archive_plan_rejects_traversal_case_collisions_and_links() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.zip");
        for name in ["../escape", "/absolute", "a\\b", "a/CON.txt", "a/../b"] {
            make_zip(&path, &[(name, b"test")]);
            assert_eq!(
                plan(&path, &FileOperationLimits::default(), 100)
                    .err()
                    .unwrap()
                    .0
                    .code,
                "unsafe_archive_path"
            );
        }
        make_zip(&path, &[("Folder/one", b"one"), ("folder/two", b"two")]);
        assert_eq!(
            plan(&path, &FileOperationLimits::default(), 100)
                .err()
                .unwrap()
                .0
                .code,
            "duplicate_archive_entry"
        );
        let mut writer = ZipWriter::new(File::create(&path).unwrap());
        writer
            .add_symlink("alias", "/private", SimpleFileOptions::default())
            .unwrap();
        writer.finish().unwrap();
        assert_eq!(
            plan(&path, &FileOperationLimits::default(), 100)
                .err()
                .unwrap()
                .0
                .code,
            "unsupported_archive_entry"
        );
    }

    #[test]
    fn archive_plan_counts_implicit_directories_before_any_publication() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.zip");
        make_zip(&path, &[("目录/子目录/file.bin", &[1, 2, 3])]);
        let (_, entries) = plan(&path, &FileOperationLimits::default(), 100).unwrap();
        assert_eq!(entries.len(), 4);
        assert!(entries[0].directory && entries[0].name.is_empty());
        let mut limits = FileOperationLimits {
            max_entries: 3,
            ..Default::default()
        };
        assert_eq!(
            plan(&path, &limits, 100).err().unwrap().0.code,
            "entry_limit"
        );
        limits.max_entries = 4;
        limits.max_bytes = 2;
        assert_eq!(
            plan(&path, &limits, 100).err().unwrap().0.code,
            "bytes_limit"
        );
    }

    #[test]
    fn corrupt_zip_content_and_cancellation_never_produce_a_complete_stage() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.zip");
        make_zip(&path, &[("file.bin", b"unique-test-content")]);
        let mut bytes = std::fs::read(&path).unwrap();
        let position = bytes
            .windows(19)
            .position(|part| part == b"unique-test-content")
            .unwrap();
        bytes[position] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        let (mut archive, _) = plan(&path, &FileOperationLimits::default(), 100).unwrap();
        let mut input = archive.by_index(0).unwrap();
        assert!(copy_bounded(&mut input, &mut Vec::new(), 19, &AtomicBool::new(false)).is_err());
        assert_eq!(
            copy_bounded(
                &mut b"abc".as_slice(),
                &mut Vec::new(),
                3,
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .0
            .code,
            "cancelled"
        );
    }
}
