use std::{
    ffi::CStr,
    os::raw::{c_char, c_int, c_void},
    ptr,
};

use base64::Engine as _;
use libloading::Library;
use pab_protocol::SystemLogSource;
use serde::Serialize;

use crate::{LogError, NativePage, NativeRecord};

const SOURCE: &str = "journal:local";
const MAX_NATIVE_RECORD_BYTES: usize = 16 * 1024 * 1024;
const MAX_SCANNED_PER_CALL: usize = 10_000;
const MAX_SCAN_BYTES: usize = 64 * 1024 * 1024;

type Open = unsafe extern "C" fn(*mut *mut c_void, c_int) -> c_int;
type Close = unsafe extern "C" fn(*mut c_void);
type SeekHead = unsafe extern "C" fn(*mut c_void) -> c_int;
type SeekCursor = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
type Next = unsafe extern "C" fn(*mut c_void) -> c_int;
type TestCursor = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
type GetCursor = unsafe extern "C" fn(*mut c_void, *mut *mut c_char) -> c_int;
type GetRealtime = unsafe extern "C" fn(*mut c_void, *mut u64) -> c_int;
type RestartData = unsafe extern "C" fn(*mut c_void);
type EnumerateData = unsafe extern "C" fn(*mut c_void, *mut *const c_void, *mut usize) -> c_int;
type SetDataThreshold = unsafe extern "C" fn(*mut c_void, usize) -> c_int;

struct Api {
    _library: Library,
    open: Open,
    close: Close,
    seek_head: SeekHead,
    seek_cursor: SeekCursor,
    next: Next,
    test_cursor: TestCursor,
    get_cursor: GetCursor,
    get_realtime: GetRealtime,
    restart_data: RestartData,
    enumerate_data: EnumerateData,
    set_data_threshold: SetDataThreshold,
}

impl Api {
    fn load() -> Result<Self, LogError> {
        // Open the versioned runtime library; no systemd development package is required.
        let library = unsafe { Library::new("libsystemd.so.0") }
            .map_err(|e| LogError::Unavailable(format!("libsystemd.so.0: {e}")))?;
        unsafe {
            macro_rules! symbol {
                ($name:literal, $ty:ty) => {
                    *library
                        .get::<$ty>(concat!($name, "\0").as_bytes())
                        .map_err(|e| LogError::Unavailable(format!("{}: {e}", $name)))?
                };
            }
            let api = Self {
                open: symbol!("sd_journal_open", Open),
                close: symbol!("sd_journal_close", Close),
                seek_head: symbol!("sd_journal_seek_head", SeekHead),
                seek_cursor: symbol!("sd_journal_seek_cursor", SeekCursor),
                next: symbol!("sd_journal_next", Next),
                test_cursor: symbol!("sd_journal_test_cursor", TestCursor),
                get_cursor: symbol!("sd_journal_get_cursor", GetCursor),
                get_realtime: symbol!("sd_journal_get_realtime_usec", GetRealtime),
                restart_data: symbol!("sd_journal_restart_data", RestartData),
                enumerate_data: symbol!("sd_journal_enumerate_data", EnumerateData),
                set_data_threshold: symbol!("sd_journal_set_data_threshold", SetDataThreshold),
                _library: library,
            };
            Ok(api)
        }
    }

    fn open_journal(&self) -> Result<Journal<'_>, LogError> {
        let mut handle = ptr::null_mut();
        let status = unsafe { (self.open)(&mut handle, 1) }; // SD_JOURNAL_LOCAL_ONLY
        if status < 0 || handle.is_null() {
            return Err(native_error("sd_journal_open", status));
        }
        let threshold = unsafe { (self.set_data_threshold)(handle, 0) };
        if threshold < 0 {
            unsafe { (self.close)(handle) };
            return Err(native_error("sd_journal_set_data_threshold", threshold));
        }
        Ok(Journal { api: self, handle })
    }
}

struct Journal<'a> {
    api: &'a Api,
    handle: *mut c_void,
}

impl Drop for Journal<'_> {
    fn drop(&mut self) {
        unsafe { (self.api.close)(self.handle) };
    }
}

fn native_error(operation: &str, status: c_int) -> LogError {
    if status == -libc::EACCES || status == -libc::EPERM {
        LogError::PermissionDenied(format!("{operation}: {}", -status))
    } else {
        LogError::Native(format!("{operation}: {}", -status))
    }
}

fn checked(operation: &str, status: c_int) -> Result<c_int, LogError> {
    if status < 0 {
        Err(native_error(operation, status))
    } else {
        Ok(status)
    }
}

pub(super) fn sources(identity: &str) -> Result<Vec<SystemLogSource>, LogError> {
    let availability = match Api::load().and_then(|api| api.open_journal().map(|_| ())) {
        Ok(()) => ("readable", None),
        Err(LogError::PermissionDenied(reason)) => ("permission_denied", Some(reason)),
        Err(error) => ("unavailable", Some(error.to_string())),
    };
    Ok(vec![SystemLogSource {
        source_id: SOURCE.into(),
        source_kind: "systemd_journal".into(),
        source_locator: "local systemd journal visible to Executor".into(),
        availability: availability.0.into(),
        reason: availability.1,
        execution_identity: identity.into(),
        coverage_limit: Some("Other journal namespaces, remote journals, container stdout and arbitrary text logs are separate sources. Journal files can change while a range is scanned; exhaustion alone does not prove a fixed snapshot.".into()),
    }])
}

#[derive(Serialize)]
struct RawField {
    name: String,
    value_base64: String,
}

fn cursor(journal: &Journal<'_>) -> Result<String, LogError> {
    let mut value = ptr::null_mut();
    checked("sd_journal_get_cursor", unsafe {
        (journal.api.get_cursor)(journal.handle, &mut value)
    })?;
    if value.is_null() {
        return Err(LogError::Native("journal cursor is null".into()));
    }
    let out = unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned();
    unsafe { libc::free(value.cast()) };
    Ok(out)
}

fn record(journal: &Journal<'_>, native_position: String) -> Result<NativeRecord, LogError> {
    let mut timestamp_usec = 0_u64;
    checked("sd_journal_get_realtime_usec", unsafe {
        (journal.api.get_realtime)(journal.handle, &mut timestamp_usec)
    })?;
    unsafe { (journal.api.restart_data)(journal.handle) };
    let mut fields = Vec::new();
    let mut message = None;
    let mut level = None;
    let mut size = 0_usize;
    loop {
        let mut data: *const c_void = ptr::null();
        let mut len = 0_usize;
        let status = checked("sd_journal_enumerate_data", unsafe {
            (journal.api.enumerate_data)(journal.handle, &mut data, &mut len)
        })?;
        if status == 0 {
            break;
        }
        if data.is_null() || len > MAX_NATIVE_RECORD_BYTES.saturating_sub(size) {
            return Err(LogError::Native(
                "journal record exceeds 16 MiB raw budget".into(),
            ));
        }
        size += len;
        let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len) };
        let Some(split) = bytes.iter().position(|b| *b == b'=') else {
            return Err(LogError::Native("journal field lacks '=' delimiter".into()));
        };
        let name = std::str::from_utf8(&bytes[..split])
            .map_err(|_| LogError::Native("journal field name is not UTF-8".into()))?;
        let value = &bytes[split + 1..];
        if name == "MESSAGE" {
            message = std::str::from_utf8(value).ok().map(str::to_owned);
        } else if name == "PRIORITY" {
            level = std::str::from_utf8(value).ok().map(str::to_owned);
        }
        fields.push(RawField {
            name: name.into(),
            value_base64: base64::engine::general_purpose::STANDARD.encode(value),
        });
    }
    let raw = serde_json::to_vec(&fields)
        .map_err(|e| LogError::Native(format!("journal serialization: {e}")))?;
    if raw.len() > MAX_NATIVE_RECORD_BYTES {
        return Err(LogError::Native(
            "journal record exceeds 16 MiB encoded budget".into(),
        ));
    }
    Ok(NativeRecord {
        source_id: SOURCE.into(),
        source_kind: "systemd_journal".into(),
        source_locator: "local systemd journal".into(),
        native_position,
        timestamp_unix_ms: i64::try_from(timestamp_usec / 1_000).unwrap_or(i64::MAX),
        native_level: level,
        raw,
        message,
    })
}

pub(super) fn scan(
    source: &str,
    after: Option<&str>,
    start_unix_ms: i64,
    end_unix_ms: i64,
    max_records: usize,
    max_bytes: usize,
) -> Result<NativePage, LogError> {
    scan_filtered(
        source,
        after,
        start_unix_ms,
        end_unix_ms,
        max_records,
        max_bytes,
        None,
    )
}

pub(super) fn scan_filtered(
    source: &str,
    after: Option<&str>,
    start_unix_ms: i64,
    end_unix_ms: i64,
    max_records: usize,
    max_bytes: usize,
    message_contains: Option<&str>,
) -> Result<NativePage, LogError> {
    if source != SOURCE {
        return Err(LogError::Unavailable(format!(
            "unknown journal source: {source}"
        )));
    }
    if max_records == 0 || max_records > 128 || max_bytes == 0 {
        return Err(LogError::Native("invalid journal page budget".into()));
    }
    let api = Api::load()?;
    let journal = api.open_journal()?;
    if let Some(previous) = after {
        let value = std::ffi::CString::new(previous)
            .map_err(|_| LogError::Native("invalid journal cursor".into()))?;
        checked("sd_journal_seek_cursor", unsafe {
            (api.seek_cursor)(journal.handle, value.as_ptr())
        })?;
        if checked("sd_journal_next", unsafe { (api.next)(journal.handle) })? == 0
            || checked("sd_journal_test_cursor", unsafe {
                (api.test_cursor)(journal.handle, value.as_ptr())
            })? == 0
        {
            return Err(LogError::Gap("journal cursor no longer exists".into()));
        }
    } else {
        checked("sd_journal_seek_head", unsafe {
            (api.seek_head)(journal.handle)
        })?;
    }
    let mut page = NativePage {
        events: Vec::new(),
        next_native_position: after.map(str::to_owned),
        scanned_count: 0,
        exhausted: false,
        // A reopened journal is not a pinned snapshot. Late writes or
        // retention between pages can invalidate a completeness claim.
        verifiable: false,
    };
    let mut bytes = 0_usize;
    let mut scanned_bytes = 0_usize;
    loop {
        let next = checked("sd_journal_next", unsafe { (api.next)(journal.handle) })?;
        if next == 0 {
            page.exhausted = true;
            break;
        }
        let position = cursor(&journal)?;
        let entry = record(&journal, position.clone())?;
        if page.scanned_count > 0 && scanned_bytes.saturating_add(entry.raw.len()) > MAX_SCAN_BYTES
        {
            break;
        }
        scanned_bytes = scanned_bytes.saturating_add(entry.raw.len());
        if entry.timestamp_unix_ms >= start_unix_ms
            && entry.timestamp_unix_ms < end_unix_ms
            && message_contains.is_none_or(|needle| {
                entry
                    .message
                    .as_deref()
                    .is_some_and(|message| message.contains(needle))
            })
        {
            if !page.events.is_empty()
                && (page.events.len() >= max_records
                    || bytes.saturating_add(entry.raw.len()) > max_bytes)
            {
                break;
            }
            bytes += entry.raw.len();
            page.events.push(entry);
        }
        page.next_native_position = Some(position);
        page.scanned_count += 1;
        if page.scanned_count as usize >= MAX_SCANNED_PER_CALL {
            break;
        }
    }
    Ok(page)
}
