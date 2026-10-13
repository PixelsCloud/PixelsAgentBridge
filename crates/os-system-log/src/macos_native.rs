//! OSLogStore's public API exposes structured entries, not the original
//! `log show` NDJSON bytes or a serializable OSLogPosition. This adapter keeps
//! those two source formats separate and never claims verified continuity.

use std::{
    collections::HashMap,
    ffi::{c_char, c_int, c_void, CStr},
    slice,
};

use base64::Engine as _;
use pab_protocol::SystemLogSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{LogError, NativePage, NativeRecord};

pub(super) const SOURCE: &str = "unified:oslogstore";
const MAX_SCANNED: usize = 10_000;
const MAX_SCAN_BYTES: usize = 64 * 1024 * 1024;
const MAX_RESUME_BYTES: usize = 64 * 1024 * 1024;
const MAX_ROW_BYTES: usize = 16 * 1024 * 1024;

type Visit =
    unsafe extern "C" fn(*const u8, usize, i64, *const c_char, *const c_char, *mut c_void) -> c_int;

unsafe extern "C" {
    fn pab_oslog_store_available(error: *mut c_char, capacity: usize) -> c_int;
    fn pab_oslog_store_visit(
        start_unix_seconds: f64,
        callback: Visit,
        context: *mut c_void,
        exhausted: *mut c_int,
        error: *mut c_char,
        capacity: usize,
    ) -> c_int;
}

#[derive(Serialize, Deserialize)]
struct Position {
    unix_us: i64,
    sha256: String,
    occurrence: u32,
}

fn encode(position: &Position) -> Result<String, LogError> {
    let bytes = serde_json::to_vec(position).map_err(|e| LogError::Native(e.to_string()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn decode(value: &str) -> Result<Position, LogError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| LogError::Gap("invalid OSLogStore position".into()))?;
    serde_json::from_slice(&bytes).map_err(|_| LogError::Gap("invalid OSLogStore position".into()))
}

fn error_text(buffer: &[c_char]) -> String {
    // The Objective-C shim always NUL-terminates this fixed-size buffer.
    unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

pub(super) fn source(identity: &str) -> SystemLogSource {
    let mut error = [0 as c_char; 512];
    let available = unsafe { pab_oslog_store_available(error.as_mut_ptr(), error.len()) } != 0;
    SystemLogSource {
        source_id: SOURCE.into(),
        source_kind: "macos_unified_oslogstore".into(),
        source_locator: "local unified log via OSLogStore (structured entry fields)".into(),
        availability: if available { "readable" } else { "unavailable" }.into(),
        reason: (!available).then(|| error_text(&error)),
        execution_identity: identity.into(),
        coverage_limit: Some("OSLogStore exposes structured fields, not original log-show NDJSON. Its position cannot be persisted across restarts; resume searches an exact row fingerprint. Retention, privacy redaction and snapshot boundaries prevent proving complete coverage, so results remain unverified. Use unified:local for log-show NDJSON bytes.".into()),
    }
}

struct Scan {
    previous: Option<Position>,
    found_previous: bool,
    occurrences: HashMap<String, u32>,
    last_us: i64,
    start_ms: i64,
    end_ms: i64,
    max_records: usize,
    max_bytes: usize,
    message_contains: Option<String>,
    page: NativePage,
    page_bytes: usize,
    scan_bytes: usize,
    resume_bytes: usize,
    error: Option<LogError>,
    reached_end: bool,
}

impl Scan {
    fn visit(
        &mut self,
        raw: &[u8],
        unix_us: i64,
        message: Option<&str>,
        level: Option<&str>,
    ) -> c_int {
        if raw.len() > MAX_ROW_BYTES {
            self.error = Some(LogError::Unverified("OSLogStore row exceeds 16 MiB".into()));
            return -1;
        }
        if self.last_us != unix_us {
            if self.last_us > unix_us {
                self.error = Some(LogError::Unverified(
                    "OSLogStore rows are out of order".into(),
                ));
                return -1;
            }
            self.occurrences.clear();
            self.last_us = unix_us;
        }
        let digest = format!("{:x}", Sha256::digest(raw));
        let occurrence = self.occurrences.entry(digest.clone()).or_default();
        *occurrence = occurrence.saturating_add(1);
        let position = Position {
            unix_us,
            sha256: digest,
            occurrence: *occurrence,
        };
        if !self.found_previous {
            self.resume_bytes = self.resume_bytes.saturating_add(raw.len());
            if self.resume_bytes > MAX_RESUME_BYTES {
                self.error = Some(LogError::Unverified(
                    "OSLogStore resume search exceeded 64 MiB".into(),
                ));
                return -1;
            }
            if self.previous.as_ref().is_some_and(|previous| {
                previous.unix_us == position.unix_us
                    && previous.sha256 == position.sha256
                    && previous.occurrence == position.occurrence
            }) {
                self.found_previous = true;
            }
            return 1;
        }
        if self.scan_bytes.saturating_add(raw.len()) > MAX_SCAN_BYTES && self.page.scanned_count > 0
        {
            return 0;
        }
        self.scan_bytes = self.scan_bytes.saturating_add(raw.len());
        if unix_us.div_euclid(1000) >= self.end_ms {
            self.reached_end = true;
            return 0;
        }
        let native_position = match encode(&position) {
            Ok(position) => position,
            Err(error) => {
                self.error = Some(error);
                return -1;
            }
        };
        if unix_us.div_euclid(1000) >= self.start_ms
            && self
                .message_contains
                .as_deref()
                .is_none_or(|needle| message.is_some_and(|message| message.contains(needle)))
        {
            if !self.page.events.is_empty()
                && (self.page.events.len() >= self.max_records
                    || self.page_bytes.saturating_add(raw.len()) > self.max_bytes)
            {
                return 0;
            }
            self.page_bytes = self.page_bytes.saturating_add(raw.len());
            self.page.events.push(NativeRecord {
                source_id: SOURCE.into(),
                source_kind: "macos_unified_oslogstore".into(),
                source_locator: "local unified log via OSLogStore (structured entry fields)".into(),
                native_position: native_position.clone(),
                timestamp_unix_ms: unix_us.div_euclid(1000),
                native_level: level.map(str::to_owned),
                raw: raw.to_vec(),
                message: message.map(str::to_owned),
            });
        }
        self.page.next_native_position = Some(native_position);
        self.page.scanned_count += 1;
        if self.page.scanned_count as usize >= MAX_SCANNED {
            0
        } else {
            1
        }
    }
}

unsafe extern "C" fn visit(
    raw: *const u8,
    length: usize,
    unix_us: i64,
    message: *const c_char,
    level: *const c_char,
    context: *mut c_void,
) -> c_int {
    let scan = unsafe { &mut *(context as *mut Scan) };
    if raw.is_null() || length > MAX_ROW_BYTES {
        scan.error = Some(LogError::Unverified("invalid OSLogStore row".into()));
        return -1;
    }
    let raw = unsafe { slice::from_raw_parts(raw, length) };
    let message = (!message.is_null())
        .then(|| unsafe { CStr::from_ptr(message) }.to_str().ok())
        .flatten();
    let level = (!level.is_null())
        .then(|| unsafe { CStr::from_ptr(level) }.to_str().ok())
        .flatten();
    scan.visit(raw, unix_us, message, level)
}

pub(super) fn scan(
    after: Option<&str>,
    start_unix_ms: i64,
    end_unix_ms: i64,
    max_records: usize,
    max_bytes: usize,
    message_contains: Option<&str>,
) -> Result<NativePage, LogError> {
    if max_records == 0 || max_records > 128 || max_bytes == 0 {
        return Err(LogError::Native("invalid OSLogStore page budget".into()));
    }
    let previous = after.map(decode).transpose()?;
    // Start slightly earlier to absorb NSDate's floating-point conversion.
    // The exact fingerprint still decides where resume begins.
    let start_us = previous
        .as_ref()
        .map_or(start_unix_ms.saturating_mul(1000), |position| {
            position.unix_us
        })
        .saturating_sub(1_000_000);
    let mut scan = Scan {
        found_previous: previous.is_none(),
        previous,
        occurrences: HashMap::new(),
        last_us: i64::MIN,
        start_ms: start_unix_ms,
        end_ms: end_unix_ms,
        max_records,
        max_bytes,
        message_contains: message_contains.map(str::to_owned),
        page: NativePage {
            events: Vec::new(),
            next_native_position: after.map(str::to_owned),
            scanned_count: 0,
            exhausted: false,
            verifiable: false,
        },
        page_bytes: 0,
        scan_bytes: 0,
        resume_bytes: 0,
        error: None,
        reached_end: false,
    };
    let mut exhausted = 0;
    let mut error = [0 as c_char; 512];
    let result = unsafe {
        pab_oslog_store_visit(
            start_us as f64 / 1_000_000.0,
            visit,
            &mut scan as *mut Scan as *mut c_void,
            &mut exhausted,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if let Some(error) = scan.error {
        return Err(error);
    }
    if result != 0 {
        return Err(LogError::Unavailable(format!(
            "OSLogStore: {}",
            error_text(&error)
        )));
    }
    if !scan.found_previous {
        return Err(LogError::Gap("OSLogStore resume row was not found".into()));
    }
    scan.page.exhausted = exhausted != 0 || scan.reached_end;
    Ok(scan.page)
}

#[cfg(test)]
mod tests {
    use std::{
        process::Command,
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[test]
    fn reads_real_unified_event_and_resumes_without_repeating_it() {
        let marker = format!(
            "PAB_OSLOGSTORE_TEST_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let start = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            - 2_000;
        let output = Command::new("/usr/bin/logger")
            .args(["-t", "PABOSLogStoreTest", &marker])
            .output()
            .expect("logger");
        assert!(output.status.success(), "logger failed: {:?}", output);
        let mut found = None;
        for _ in 0..20 {
            let mut after = None;
            for _ in 0..6 {
                let page = scan(
                    after.as_deref(),
                    start,
                    i64::MAX,
                    128,
                    1024 * 1024,
                    Some(&marker),
                )
                .expect("OSLogStore scan");
                if let Some(record) = page
                    .events
                    .into_iter()
                    .find(|record| record.message.as_deref() == Some(&marker))
                {
                    found = Some(record);
                    break;
                }
                if page.exhausted {
                    break;
                }
                after = page.next_native_position;
            }
            if found.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(300));
        }
        let record = found.expect("logger event absent from OSLogStore");
        assert_eq!(record.source_id, SOURCE);
        let fields: serde_json::Value = serde_json::from_slice(&record.raw).unwrap();
        assert_eq!(fields["source"], "OSLogStore");
        assert_eq!(fields["eventMessage"], marker);
        let next = scan(
            Some(&record.native_position),
            start,
            i64::MAX,
            128,
            1024 * 1024,
            Some(&marker),
        )
        .expect("resume from native position");
        assert!(next
            .events
            .iter()
            .all(|event| event.message.as_deref() != Some(&marker)));
        assert!(!next.verifiable);

        let mut lost = decode(&record.native_position).unwrap();
        lost.sha256 = "0".repeat(64);
        let lost = encode(&lost).unwrap();
        assert!(matches!(
            scan(
                Some(&lost),
                start,
                i64::MAX,
                128,
                1024 * 1024,
                Some(&marker)
            ),
            Err(LogError::Gap(_))
        ));
    }
}
