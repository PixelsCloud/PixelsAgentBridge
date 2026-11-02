use super::*;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    os::windows::ffi::OsStringExt,
    path::Path,
    sync::Arc,
};
fn error(code: &'static str) -> io::Error {
    io::Error::other(code)
}
#[repr(C)]
struct Descriptor {
    name: [u16; 260],
    size: u64,
    modified: u64,
    directory: u32,
}
#[repr(C)]
#[derive(Default)]
struct Range {
    id: u64,
    stream: u64,
    batch: u64,
    offset: u64,
    index: u32,
    length: u32,
    end: u32,
    offer: [u8; 16],
}
unsafe extern "C" {
    fn pab_fc_new() -> *mut std::ffi::c_void;
    fn pab_fc_free(context: *mut std::ffi::c_void);
    fn pab_fc_sources(
        context: *mut std::ffi::c_void,
        paths: *mut u16,
        capacity: u32,
        sequence: *mut u64,
    ) -> i32;
    fn pab_fc_publish(
        context: *mut std::ffi::c_void,
        id: *const u8,
        files: *const Descriptor,
        count: u32,
        expected: u64,
    ) -> i32;
    fn pab_fc_poll(context: *mut std::ffi::c_void, request: *mut Range) -> i32;
    fn pab_fc_reply(
        context: *mut std::ffi::c_void,
        id: u64,
        data: *const u8,
        length: u32,
        failed: i32,
    ) -> i32;
    fn pab_fc_clear(context: *mut std::ffi::c_void);
    fn pab_fc_withdraw(context: *mut std::ffi::c_void, id: *const u8);
}
struct Context(*mut std::ffi::c_void);
impl Drop for Context {
    fn drop(&mut self) {
        unsafe { pab_fc_free(self.0) }
    }
}
struct Source {
    path: PathBuf,
    identity: FilePathIdentity,
    size: u64,
    modified: Option<i64>,
}
struct Offer {
    header: ClipboardFileOffer,
    sources: Vec<Source>,
}
struct Lease {
    batch: u64,
    offer: Arc<Offer>,
    index: u32,
    file: File,
    touched: std::time::Instant,
}
struct BatchLease {
    offer: Arc<Offer>,
    touched: std::time::Instant,
}
pub(super) struct State {
    context: Context,
    sequence: Option<u64>,
    current: Option<Arc<Offer>>,
    leases: HashMap<(RequestId, u64), Lease>,
    batches: HashMap<(RequestId, u64), BatchLease>,
    paths: Vec<u16>,
    imported: bool,
    last_error: Option<String>,
}
impl State {
    fn begin_batch(
        &mut self,
        request: &ClipboardFileRead,
        observed_sequence: u64,
    ) -> io::Result<()> {
        let key = (request.offer, request.batch);
        if self.batches.contains_key(&key) {
            return Ok(());
        }
        if self.batches.len() >= 8 {
            return Err(error("file_clipboard_busy"));
        }
        if self.sequence != Some(observed_sequence) {
            return Err(error("file_clipboard_expired"));
        }
        let offer = self
            .current
            .as_ref()
            .filter(|s| s.header.id == request.offer)
            .cloned()
            .ok_or_else(|| error("file_clipboard_expired"))?;
        self.batches.insert(
            key,
            BatchLease {
                offer,
                touched: std::time::Instant::now(),
            },
        );
        Ok(())
    }
    pub(super) fn new() -> io::Result<Self> {
        let raw = unsafe { pab_fc_new() };
        if raw.is_null() {
            return Err(error("file_clipboard_unavailable"));
        }
        Ok(Self {
            context: Context(raw),
            sequence: None,
            current: None,
            leases: HashMap::new(),
            batches: HashMap::new(),
            paths: vec![0; 2 * 1024 * 1024],
            imported: false,
            last_error: None,
        })
    }
    fn poll(
        &mut self,
        after: Option<u64>,
        take_requests: bool,
    ) -> io::Result<FileClipboardSnapshot> {
        self.batches
            .retain(|_, batch| batch.touched.elapsed() < std::time::Duration::from_secs(30));
        let batches = &self.batches;
        self.leases.retain(|(offer, _), lease| {
            lease.touched.elapsed() < std::time::Duration::from_secs(30)
                && batches.contains_key(&(*offer, lease.batch))
        });
        let mut sequence = self.sequence.unwrap_or(u64::MAX);
        let status = unsafe {
            pab_fc_sources(
                self.context.0,
                self.paths.as_mut_ptr(),
                self.paths.len() as u32,
                &mut sequence,
            )
        };
        let changed = self.sequence != Some(sequence);
        if status != -5 {
            self.imported = status == -2;
        }
        let mut snapshot = FileClipboardSnapshot {
            sequence,
            changed: after != Some(sequence),
            imported: self.imported,
            ..Default::default()
        };
        if status == -1 {
            return Err(error("file_clipboard_busy"));
        }
        if changed {
            self.current = None;
            self.last_error = None;
            if status > 0 {
                let roots = self.paths[..status as usize]
                    .split(|c| *c == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| PathBuf::from(std::ffi::OsString::from_wide(s)))
                    .collect::<Vec<_>>();
                match inventory(roots) {
                    Ok(offer) => self.current = Some(Arc::new(offer)),
                    Err(_) => self.last_error = Some("file_clipboard_source_unavailable".into()),
                }
            }
            self.sequence = Some(sequence);
        }
        if status == -3 {
            self.last_error = Some("file_clipboard_cut_unsupported".into());
        } else if status == -4 {
            self.last_error = Some("file_clipboard_too_many_files".into());
        }
        snapshot.error = self.last_error.clone();
        if snapshot.changed {
            snapshot.offer = self.current.as_ref().map(|s| s.header.clone());
        }
        // Mutation replies acknowledge a clipboard revision only. Their callers
        // must not accidentally consume outstanding stream/batch requests.
        for _ in 0..if take_requests { 32 } else { 0 } {
            let mut r = Range::default();
            if unsafe { pab_fc_poll(self.context.0, &mut r) } != 1 {
                break;
            }
            snapshot.requests.push(ClipboardFileRead {
                id: r.id,
                stream: r.stream,
                batch: r.batch,
                batch_event: r.end >= 2,
                offer: RequestId::from_u128(u128::from_be_bytes(r.offer)),
                index: r.index,
                offset: r.offset,
                length: r.length,
                end: r.end == 1 || r.end == 3,
            });
        }
        Ok(snapshot)
    }
    pub(super) fn exchange(
        &mut self,
        operation: FileClipboardOperation,
    ) -> io::Result<FileClipboardSnapshot> {
        operation.validate().map_err(io::Error::other)?;
        match operation {
            FileClipboardOperation::Poll { after } => self.poll(after, true),
            FileClipboardOperation::Publish {
                offer,
                expected_sequence,
            } => {
                let descriptors = offer
                    .files
                    .iter()
                    .map(|f| {
                        let mut name = [0; 260];
                        let units = f.name.encode_utf16().collect::<Vec<_>>();
                        name[..units.len()].copy_from_slice(&units);
                        Descriptor {
                            name,
                            size: f.size,
                            modified: f.modified,
                            directory: u32::from(f.directory),
                        }
                    })
                    .collect::<Vec<_>>();
                let code = unsafe {
                    pab_fc_publish(
                        self.context.0,
                        offer.id.as_uuid().as_bytes().as_ptr(),
                        descriptors.as_ptr(),
                        descriptors.len() as u32,
                        expected_sequence.unwrap_or(u64::MAX),
                    )
                };
                if code != 0 {
                    return Err(error(if code == -2 {
                        "file_clipboard_changed"
                    } else {
                        "file_clipboard_write_failed"
                    }));
                }
                self.poll(None, false)
            }
            FileClipboardOperation::Read { request } => {
                let batch_key = (request.offer, request.batch);
                if request.batch_event {
                    if request.end {
                        self.batches.remove(&batch_key);
                        self.leases.retain(|(offer, _), lease| {
                            *offer != request.offer || lease.batch != request.batch
                        });
                    } else {
                        self.begin_batch(&request, unsafe {
                            windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber()
                        } as u64)?;
                    }
                    return Ok(FileClipboardSnapshot::default());
                }
                let key = (request.offer, request.stream);
                if request.end {
                    self.leases.remove(&key);
                    return Ok(FileClipboardSnapshot::default());
                }
                let batch = self
                    .batches
                    .get_mut(&batch_key)
                    .ok_or_else(|| error("file_clipboard_expired"))?;
                batch.touched = std::time::Instant::now();
                if !self.leases.contains_key(&key) {
                    if self.leases.len() >= 4 {
                        return Err(error("file_clipboard_busy"));
                    }
                    let offer = batch.offer.clone();
                    let source = offer
                        .sources
                        .get(request.index as usize)
                        .ok_or_else(|| error("file_clipboard_invalid_range"))?;
                    let current = pab_file_manager::inspect_path_identity(
                        &pab_file_manager::encode_path(&source.path),
                    )
                    .map_err(|_| error("file_clipboard_source_changed"))?;
                    if current != source.identity {
                        return Err(error("file_clipboard_source_changed"));
                    }
                    let file = pab_file_manager::open_source(&source.path)
                        .map_err(|_| error("file_clipboard_source_unavailable"))?;
                    if Some(pab_os_control::filesystem::file_object_identity(&file)?)
                        != source.identity.object
                    {
                        return Err(error("file_clipboard_source_changed"));
                    }
                    pab_file_manager::verify_source_metadata(&file, source.size, source.modified)
                        .map_err(|_| error("file_clipboard_source_changed"))?;
                    self.leases.insert(
                        key,
                        Lease {
                            batch: request.batch,
                            offer,
                            index: request.index,
                            file,
                            touched: std::time::Instant::now(),
                        },
                    );
                }
                let lease = self.leases.get_mut(&key).unwrap();
                let source = &lease.offer.sources[lease.index as usize];
                lease.touched = std::time::Instant::now();
                if lease.index != request.index
                    || lease.batch != request.batch
                    || request
                        .offset
                        .checked_add(request.length as u64)
                        .is_none_or(|end| end > source.size)
                {
                    return Err(error("file_clipboard_invalid_range"));
                }
                pab_file_manager::verify_source_metadata(&lease.file, source.size, source.modified)
                    .map_err(|_| error("file_clipboard_source_changed"))?;
                lease.file.seek(SeekFrom::Start(request.offset))?;
                let mut data = vec![0; request.length as usize];
                lease.file.read_exact(&mut data)?;
                Ok(FileClipboardSnapshot {
                    bytes: data.len() as u32,
                    sha256: Some(format!("{:x}", Sha256::digest(&data))),
                    data,
                    ..Default::default()
                })
            }
            FileClipboardOperation::Deliver {
                id,
                bytes,
                error,
                data,
            } => {
                if data.len() != bytes as usize {
                    return Err(error_fn());
                }
                if unsafe {
                    pab_fc_reply(
                        self.context.0,
                        id,
                        data.as_ptr(),
                        bytes,
                        i32::from(error.is_some()),
                    )
                } != 0
                {
                    return Err(error_fn());
                }
                Ok(FileClipboardSnapshot::default())
            }
            FileClipboardOperation::Clear => {
                unsafe { pab_fc_clear(self.context.0) };
                self.current = None;
                self.leases.clear();
                self.batches.clear();
                self.sequence = None;
                self.last_error = None;
                self.imported = false;
                Ok(FileClipboardSnapshot::default())
            }
            FileClipboardOperation::Withdraw { offer } => {
                unsafe { pab_fc_withdraw(self.context.0, offer.as_uuid().as_bytes().as_ptr()) };
                self.poll(None, false)
            }
        }
    }
}
fn error_fn() -> io::Error {
    error("file_clipboard_invalid_reply")
}
fn inventory(roots: Vec<PathBuf>) -> io::Result<Offer> {
    let mut offer = Offer {
        header: ClipboardFileOffer {
            id: RequestId::new(),
            files: vec![],
        },
        sources: vec![],
    };
    fn visit(offer: &mut Offer, path: &Path, name: String, depth: usize) -> io::Result<()> {
        if name.encode_utf16().count() > 259 {
            return Err(error("file_clipboard_invalid_name"));
        }
        if depth > 128 || offer.sources.len() >= FILE_CLIPBOARD_ITEMS {
            return Err(error("file_clipboard_too_many_files"));
        }
        let identity =
            pab_file_manager::inspect_path_identity(&pab_file_manager::encode_path(path))
                .map_err(|_| error("file_clipboard_source_unavailable"))?;
        let mut metadata = std::fs::symlink_metadata(path)?;
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 || !metadata.is_file() && !metadata.is_dir() {
            return Err(error("file_clipboard_source_unavailable"));
        }
        if metadata.is_file() {
            let file = pab_file_manager::open_source(path)?;
            if Some(pab_os_control::filesystem::file_object_identity(&file)?) != identity.object {
                return Err(error("file_clipboard_source_changed"));
            }
            metadata = file.metadata()?;
        }
        let modified = metadata.last_write_time();
        let directory = metadata.is_dir();
        offer.header.files.push(ClipboardFile {
            name: name.clone(),
            directory,
            size: if directory { 0 } else { metadata.len() },
            modified,
        });
        offer.sources.push(Source {
            path: path.to_owned(),
            identity: identity.clone(),
            size: metadata.len(),
            modified: pab_file_manager::modified_at_unix_ms(&metadata),
        });
        if directory {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let child = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| error("file_clipboard_invalid_name"))?;
                visit(offer, &entry.path(), format!("{name}\\{child}"), depth + 1)?;
            }
            if pab_file_manager::inspect_path_identity(&pab_file_manager::encode_path(path))
                .map_err(io::Error::other)?
                != identity
            {
                return Err(error("file_clipboard_source_changed"));
            }
        }
        Ok(())
    }
    let native = roots
        .iter()
        .map(|p| pab_file_manager::encode_path(p))
        .collect::<Vec<_>>();
    let independent = pab_file_manager::independent_sources(&native, PathStyle::Windows)
        .map_err(io::Error::other)?;
    for root in independent {
        let path = pab_file_manager::decode_path(&root).map_err(io::Error::other)?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| error("file_clipboard_invalid_name"))?
            .to_owned();
        visit(&mut offer, &path, name, 0)?;
    }
    offer.header.validate().map_err(io::Error::other)?;
    Ok(offer)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Filesystem-only: never opens or changes the control host's clipboard.
    #[test]
    fn paste_batch_keeps_remaining_files_after_a_new_copy_and_releases_at_end() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.build/file-clipboard-20261031/source-tests")
            .join(RequestId::new().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let first = root.join("first.txt");
        let second = root.join("后续.txt");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let offer = Arc::new(inventory(vec![first.clone(), second.clone()]).unwrap());
        let id = offer.header.id;
        let mut state = State {
            context: Context(std::ptr::null_mut()),
            sequence: Some(1),
            current: Some(offer),
            leases: HashMap::new(),
            batches: HashMap::new(),
            paths: vec![],
            imported: false,
            last_error: None,
        };
        let mut request = ClipboardFileRead {
            id: 1,
            stream: 10,
            batch: 20,
            batch_event: true,
            offer: id,
            index: 0,
            offset: 0,
            length: 0,
            end: false,
        };
        state.begin_batch(&request, 1).unwrap();
        request.batch_event = false;
        request.length = 5;
        assert_eq!(
            state
                .exchange(FileClipboardOperation::Read {
                    request: request.clone()
                })
                .unwrap()
                .data,
            b"first"
        );
        state.current = None;
        state.sequence = Some(2);
        request.end = true;
        state
            .exchange(FileClipboardOperation::Read {
                request: request.clone(),
            })
            .unwrap();
        assert!(state.leases.is_empty());
        request.end = false;
        request.stream = 11;
        request.index = 1;
        request.length = 6;
        assert_eq!(
            state
                .exchange(FileClipboardOperation::Read {
                    request: request.clone()
                })
                .unwrap()
                .data,
            b"second"
        );
        request.batch_event = true;
        request.end = true;
        state
            .exchange(FileClipboardOperation::Read {
                request: request.clone(),
            })
            .unwrap();
        assert!(state.leases.is_empty() && state.batches.is_empty());
        request.end = false;
        assert!(state.begin_batch(&request, 2).is_err());
        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
