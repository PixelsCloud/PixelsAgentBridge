//! macOS unified-log CLI source through the platform's `log show` reader.
//! A CLI row has no durable OSLogStore position. Resume re-finds the exact
//! previous raw row within its second; any ambiguity or loss is a gap, and
//! even an exhausted range is deliberately reported as unverified.

use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
};

use base64::Engine as _;
use chrono::DateTime;
use pab_protocol::SystemLogSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{LogError, NativePage, NativeRecord};

#[path = "macos_native.rs"]
mod native;

const SOURCE: &str = "unified:local";
const MAX_LINE: usize = 16 * 1024 * 1024;
const MAX_SCANNED: usize = 10_000;
const MAX_RESUME_BYTES: usize = 64 * 1024 * 1024;
const MAX_SCAN_BYTES: usize = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Position {
    second: i64,
    sha256: String,
    occurrence: u32,
}

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(super) fn sources(identity: &str) -> Result<Vec<SystemLogSource>, LogError> {
    let readable = std::path::Path::new("/usr/bin/log").exists();
    Ok(vec![native::source(identity), SystemLogSource {
        source_id: SOURCE.into(),
        source_kind: "macos_unified_log".into(),
        source_locator: "local unified log through /usr/bin/log show".into(),
        availability: if readable { "readable" } else { "unavailable" }.into(),
        reason: (!readable).then(|| "/usr/bin/log is unavailable".into()),
        execution_identity: identity.into(),
        coverage_limit: Some("macOS retention, privacy redaction, non-persisted debug/info events, and CLI row ordering limit verification; every exhausted scan remains unverified. Use a command for sources outside the unified log.".into()),
    }])
}

fn encode_position(position: &Position) -> Result<String, LogError> {
    let raw = serde_json::to_vec(position).map_err(|e| LogError::Native(e.to_string()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw))
}

fn decode_position(value: &str) -> Result<Position, LogError> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| LogError::Gap("invalid unified-log position".into()))?;
    serde_json::from_slice(&raw).map_err(|_| LogError::Gap("invalid unified-log position".into()))
}

fn bounded_line<R: BufRead>(input: &mut R) -> Result<Option<Vec<u8>>, LogError> {
    let mut line = Vec::new();
    loop {
        let buf = input
            .fill_buf()
            .map_err(|e| LogError::Native(e.to_string()))?;
        if buf.is_empty() {
            return Ok((!line.is_empty()).then_some(line));
        }
        let len = buf
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buf.len(), |n| n + 1);
        if line.len().saturating_add(len) > MAX_LINE {
            return Err(LogError::Unverified(
                "unified-log row exceeds 16 MiB".into(),
            ));
        }
        line.extend_from_slice(&buf[..len]);
        input.consume(len);
        if line.last() == Some(&b'\n') {
            return Ok(Some(line));
        }
    }
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
    if source == native::SOURCE {
        return native::scan(
            after,
            start_unix_ms,
            end_unix_ms,
            max_records,
            max_bytes,
            message_contains,
        );
    }
    if source != SOURCE {
        return Err(LogError::Unavailable("unknown unified-log source".into()));
    }
    if max_records == 0 || max_records > 128 || max_bytes == 0 {
        return Err(LogError::Native("invalid unified-log page budget".into()));
    }
    let previous = after.map(decode_position).transpose()?;
    let start_second = previous
        .as_ref()
        .map_or(start_unix_ms.div_euclid(1000), |p| p.second);
    let mut command = Command::new("/usr/bin/log");
    command
        .args([
            "show",
            "--style",
            "ndjson",
            "--info",
            "--debug",
            "--loss",
            "--no-pager",
            "--start",
        ])
        .arg(format!("@{start_second}"));
    if end_unix_ms != i64::MAX {
        command.args([
            "--end",
            &format!("@{}", end_unix_ms.saturating_add(999).div_euclid(1000)),
        ]);
    }
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| LogError::Unavailable(format!("log show: {e}")))?;
    let mut process = Process(child);
    let stdout = process
        .0
        .stdout
        .take()
        .ok_or_else(|| LogError::Native("log show stdout unavailable".into()))?;
    let mut input = BufReader::new(stdout);
    let mut found_previous = previous.is_none();
    let mut occurrences = std::collections::HashMap::<String, u32>::new();
    let mut last_second = i64::MIN;
    let mut page = NativePage {
        events: Vec::new(),
        next_native_position: after.map(str::to_owned),
        scanned_count: 0,
        exhausted: false,
        verifiable: false,
    };
    let mut bytes = 0_usize;
    let mut stopped_early = false;
    let mut row_count = 0_u64;
    let mut summary_count = None;
    let mut resume_bytes = 0_usize;
    let mut scan_bytes = 0_usize;
    while let Some(mut raw) = bounded_line(&mut input)? {
        if raw.last() == Some(&b'\n') {
            raw.pop();
        }
        if raw.last() == Some(&b'\r') {
            raw.pop();
        }
        if raw.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(&raw)
            .map_err(|e| LogError::Unverified(format!("unified-log row is not NDJSON: {e}")))?;
        let Some(timestamp) = value.get("timestamp").and_then(|v| v.as_str()) else {
            // `log show --style ndjson` emits one terminal summary object in
            // addition to event rows. It has no timestamp and is not an event.
            if value.as_object().is_some_and(|object| object.len() == 2)
                && value.get("finished").and_then(|v| v.as_u64()) == Some(1)
            {
                summary_count = value.get("count").and_then(|v| v.as_u64());
                continue;
            }
            return Err(LogError::Unverified("unified-log timestamp missing".into()));
        };
        row_count = row_count.saturating_add(1);
        let timestamp_ms = DateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S%.f%z")
            .map_err(|e| LogError::Unverified(format!("unified-log timestamp: {e}")))?
            .timestamp_millis();
        let second = timestamp_ms.div_euclid(1000);
        let digest = format!("{:x}", Sha256::digest(&raw));
        if second != last_second {
            occurrences.clear();
            last_second = second;
        }
        let occurrence = occurrences.entry(digest.clone()).or_default();
        *occurrence = occurrence.saturating_add(1);
        let position = Position {
            second,
            sha256: digest,
            occurrence: *occurrence,
        };
        if !found_previous {
            resume_bytes = resume_bytes.saturating_add(raw.len());
            if resume_bytes > MAX_RESUME_BYTES {
                return Err(LogError::Unverified(
                    "unified-log resume search exceeded 64 MiB".into(),
                ));
            }
            if previous.as_ref().is_some_and(|p| {
                p.second == position.second
                    && p.sha256 == position.sha256
                    && p.occurrence == position.occurrence
            }) {
                found_previous = true;
            }
            continue;
        }
        if scan_bytes.saturating_add(raw.len()) > MAX_SCAN_BYTES && page.scanned_count > 0 {
            stopped_early = true;
            break;
        }
        scan_bytes = scan_bytes.saturating_add(raw.len());
        if timestamp_ms < start_unix_ms || timestamp_ms >= end_unix_ms {
            page.next_native_position = Some(encode_position(&position)?);
            page.scanned_count += 1;
            if page.scanned_count as usize >= MAX_SCANNED {
                stopped_early = true;
                break;
            }
            continue;
        }
        if message_contains.is_some_and(|needle| {
            !value
                .get("eventMessage")
                .and_then(|v| v.as_str())
                .is_some_and(|message| message.contains(needle))
        }) {
            page.next_native_position = Some(encode_position(&position)?);
            page.scanned_count += 1;
            if page.scanned_count as usize >= MAX_SCANNED {
                stopped_early = true;
                break;
            }
            continue;
        }
        if !page.events.is_empty()
            && (page.events.len() >= max_records || bytes.saturating_add(raw.len()) > max_bytes)
        {
            stopped_early = true;
            break;
        }
        let native_position = encode_position(&position)?;
        bytes += raw.len();
        page.events.push(NativeRecord {
            source_id: SOURCE.into(),
            source_kind: "macos_unified_log".into(),
            source_locator: "local unified log".into(),
            native_position: native_position.clone(),
            timestamp_unix_ms: timestamp_ms,
            native_level: value
                .get("messageType")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            message: value
                .get("eventMessage")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            raw,
        });
        page.next_native_position = Some(native_position);
        page.scanned_count += 1;
        if page.scanned_count as usize >= MAX_SCANNED {
            stopped_early = true;
            break;
        }
    }
    if !found_previous {
        return Err(LogError::Gap("unified-log resume row was not found".into()));
    }
    if stopped_early {
        return Ok(page);
    }
    drop(input);
    let status = process
        .0
        .wait()
        .map_err(|e| LogError::Native(format!("log show wait: {e}")))?;
    if !status.success() {
        return Err(LogError::Unverified(format!(
            "log show exited with {status}"
        )));
    }
    if summary_count != Some(row_count) {
        return Err(LogError::Unverified(format!(
            "log show row count differs from terminal summary: {row_count} vs {summary_count:?}"
        )));
    }
    page.exhausted = true;
    Ok(page)
}
