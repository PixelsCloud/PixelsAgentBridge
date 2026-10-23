//! Exact physical Pixels log files in the Executor's configured log directory.
//! Rotation is reported as a source change. A missing generation or identity
//! mismatch is a gap, never silently resumed at the same path.

use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::Path,
};

use base64::Engine as _;
use chrono::DateTime;
use pab_protocol::SystemLogSource;
use serde::{Deserialize, Serialize};

use crate::{LogError, NativePage, NativeRecord};

const PREFIX: &str = "pixels-file:";
const MAX_LINE: usize = 16 * 1024 * 1024;
const MAX_SCANNED: usize = 10_000;
const ROLES: [&str; 8] = [
    "executor",
    "desktop",
    "mcp",
    "bridge",
    "enroll",
    "session-supervisor",
    "session-helper",
    "login-helper",
];

#[derive(Serialize, Deserialize)]
struct Position {
    identity: String,
    offset: u64,
}

fn file_name(source: &str) -> Result<&str, LogError> {
    let filename = source
        .strip_prefix(PREFIX)
        .ok_or_else(|| LogError::Unavailable("invalid Pixels log source".into()))?;
    if !ROLES.iter().any(|role| {
        filename == format!("{role}.log")
            || (1..=4).any(|index| filename == format!("{role}.log.{index}"))
    }) {
        return Err(LogError::Unavailable("unknown Pixels log file".into()));
    }
    Ok(filename)
}

fn file_identity(file: &File) -> Result<String, LogError> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(LogError::Unverified(
                "Windows file identity unavailable".into(),
            ));
        }
        return Ok(format!(
            "{}:{}",
            info.dwVolumeSerialNumber,
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file
            .metadata()
            .map_err(|e| LogError::Native(e.to_string()))?;
        return Ok(format!("{}:{}", metadata.dev(), metadata.ino()));
    }
    #[allow(unreachable_code)]
    Err(LogError::Unavailable(
        "unsupported file identity platform".into(),
    ))
}

fn position(value: &Position) -> Result<String, LogError> {
    let raw = serde_json::to_vec(value).map_err(|e| LogError::Native(e.to_string()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw))
}

fn parse_position(value: &str) -> Result<Position, LogError> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| LogError::Gap("invalid Pixels log offset".into()))?;
    serde_json::from_slice(&raw).map_err(|_| LogError::Gap("invalid Pixels log offset".into()))
}

pub fn sources(root: &Path, identity: &str) -> Result<Vec<SystemLogSource>, LogError> {
    let files = match fs::read_dir(root) {
        Ok(files) => files,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(LogError::PermissionDenied(format!(
                "Pixels log directory: {error}"
            )));
        }
        Err(error) => return Err(LogError::Native(format!("Pixels log directory: {error}"))),
    };
    let mut sources = Vec::new();
    for file in files {
        let file = file.map_err(|e| LogError::Native(e.to_string()))?;
        let name = file.file_name().to_string_lossy().into_owned();
        if file_name(&format!("{PREFIX}{name}")).is_err() {
            continue;
        }
        let metadata =
            fs::symlink_metadata(file.path()).map_err(|e| LogError::Native(e.to_string()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        sources.push(SystemLogSource {
            source_id: format!("{PREFIX}{name}"), source_kind: "pixels_rotating_file".into(),
            source_locator: file.path().to_string_lossy().into_owned(),
            availability: "readable".into(), reason: None, execution_identity: identity.into(),
            coverage_limit: Some("One physical file generation only. Rotation changes its path; a lost generation reports a gap. Only Pixels log files in the Executor-configured directory are listed, not other user profiles or arbitrary files. Incomplete trailing lines wait for completion.".into()),
        });
    }
    sources.sort_by(|a, b| a.source_id.cmp(&b.source_id));
    Ok(sources)
}

fn bounded_line<R: BufRead>(reader: &mut R) -> Result<(Vec<u8>, bool), LogError> {
    let mut line = Vec::new();
    loop {
        let chunk = reader
            .fill_buf()
            .map_err(|e| LogError::Native(e.to_string()))?;
        if chunk.is_empty() {
            return Ok((line, false));
        }
        let len = chunk
            .iter()
            .position(|b| *b == b'\n')
            .map_or(chunk.len(), |n| n + 1);
        if line.len().saturating_add(len) > MAX_LINE {
            return Err(LogError::Unverified(
                "Pixels log line exceeds 16 MiB".into(),
            ));
        }
        line.extend_from_slice(&chunk[..len]);
        reader.consume(len);
        if line.last() == Some(&b'\n') {
            return Ok((line, true));
        }
    }
}

pub fn scan(
    root: &Path,
    source: &str,
    after: Option<&str>,
    start_unix_ms: i64,
    end_unix_ms: i64,
    max_records: usize,
    max_bytes: usize,
) -> Result<NativePage, LogError> {
    let name = file_name(source)?;
    if max_records == 0 || max_records > 128 || max_bytes == 0 {
        return Err(LogError::Native("invalid Pixels log page budget".into()));
    }
    let path = root.join(name);
    let mut file = File::open(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            LogError::Gap("Pixels log generation no longer exists".into())
        } else {
            LogError::Native(format!("Pixels log open: {e}"))
        }
    })?;
    let identity = file_identity(&file)?;
    let offset = if let Some(previous) = after {
        let previous = parse_position(previous)?;
        if previous.identity != identity {
            return Err(LogError::Gap(
                "Pixels log file was replaced or rotated".into(),
            ));
        }
        previous.offset
    } else {
        0
    };
    if offset
        > file
            .metadata()
            .map_err(|e| LogError::Native(e.to_string()))?
            .len()
    {
        return Err(LogError::Gap("Pixels log file was truncated".into()));
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| LogError::Native(e.to_string()))?;
    let mut reader = BufReader::new(file);
    let mut page = NativePage {
        events: Vec::new(),
        next_native_position: after.map(str::to_owned),
        scanned_count: 0,
        exhausted: false,
        verifiable: false,
    };
    let mut bytes = 0_usize;
    loop {
        let before = reader
            .stream_position()
            .map_err(|e| LogError::Native(e.to_string()))?;
        let (mut raw, complete) = bounded_line(&mut reader)?;
        if !complete {
            page.exhausted = true;
            break;
        }
        let text = std::str::from_utf8(&raw)
            .map_err(|_| LogError::Unverified("Pixels log line is not UTF-8".into()))?;
        let timestamp = text
            .split_whitespace()
            .next()
            .ok_or_else(|| LogError::Unverified("Pixels log timestamp missing".into()))?;
        let timestamp = DateTime::parse_from_rfc3339(timestamp)
            .map_err(|_| LogError::Unverified("Pixels log timestamp is invalid".into()))?
            .timestamp_millis();
        let native_level = text.split_whitespace().nth(1).map(str::to_owned);
        // Tracing messages and captured media stderr can contain continuation
        // lines without a timestamp. Keep those bytes in their preceding record
        // instead of aborting the scan and losing all later timestamped events.
        // The cursor always lands at a record boundary, including after paging.
        loop {
            let next_start = reader
                .stream_position()
                .map_err(|e| LogError::Native(e.to_string()))?;
            let (next, complete) = bounded_line(&mut reader)?;
            let begins_record = std::str::from_utf8(&next)
                .ok()
                .and_then(|line| line.split_whitespace().next())
                .is_some_and(|timestamp| DateTime::parse_from_rfc3339(timestamp).is_ok());
            if !complete || begins_record {
                reader
                    .seek(SeekFrom::Start(next_start))
                    .map_err(|e| LogError::Native(e.to_string()))?;
                break;
            }
            if raw.len().saturating_add(next.len()) > MAX_LINE {
                return Err(LogError::Unverified(
                    "Pixels log record exceeds 16 MiB".into(),
                ));
            }
            raw.extend_from_slice(&next);
        }
        let text = std::str::from_utf8(&raw)
            .map_err(|_| LogError::Unverified("Pixels log record is not UTF-8".into()))?;
        let after_offset = before.saturating_add(raw.len() as u64);
        if timestamp >= start_unix_ms && timestamp < end_unix_ms {
            if !page.events.is_empty()
                && (page.events.len() >= max_records || bytes.saturating_add(raw.len()) > max_bytes)
            {
                break;
            }
            bytes += raw.len();
            let native_position = position(&Position {
                identity: identity.clone(),
                offset: after_offset,
            })?;
            page.events.push(NativeRecord {
                source_id: source.into(),
                source_kind: "pixels_rotating_file".into(),
                source_locator: path.to_string_lossy().into_owned(),
                native_position: native_position.clone(),
                timestamp_unix_ms: timestamp,
                native_level,
                message: Some(text.to_owned()),
                raw,
            });
        }
        page.next_native_position = Some(position(&Position {
            identity: identity.clone(),
            offset: after_offset,
        })?);
        page.scanned_count += 1;
        if page.scanned_count as usize >= MAX_SCANNED {
            break;
        }
    }
    let current = File::open(&path)
        .map_err(|_| LogError::Gap("Pixels log path disappeared during scan".into()))?;
    if file_identity(&current)? != identity {
        return Err(LogError::Gap("Pixels log rotated during scan".into()));
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiline_media_errors_preserve_raw_bytes_and_resume_at_next_record() {
        let root = tempfile::tempdir().unwrap();
        let first = "2026-10-23T03:00:00Z ERROR capture failed\nmedia: Screen capture stopped: 找不到显示器\n  caused by: unavailable window\n";
        let second = "2026-10-23T04:00:00Z INFO new event\n";
        fs::write(root.path().join("executor.log"), format!("{first}{second}")).unwrap();
        let page = scan(
            root.path(),
            "pixels-file:executor.log",
            None,
            0,
            i64::MAX,
            1,
            4096,
        )
        .unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].raw, first.as_bytes());
        assert_eq!(page.events[0].native_level.as_deref(), Some("ERROR"));
        let next = scan(
            root.path(),
            "pixels-file:executor.log",
            page.next_native_position.as_deref(),
            0,
            i64::MAX,
            1,
            4096,
        )
        .unwrap();
        assert_eq!(next.events.len(), 1);
        assert_eq!(next.events[0].raw, second.as_bytes());
        assert!(next.exhausted);
        // Filtering out the old multiline event must still reach recent logs.
        let recent = scan(
            root.path(),
            "pixels-file:executor.log",
            None,
            1792728000000,
            i64::MAX,
            1,
            4096,
        )
        .unwrap();
        assert_eq!(recent.events.len(), 1);
        assert_eq!(recent.events[0].raw, second.as_bytes());
    }

    #[test]
    fn standalone_unstructured_prefix_is_explicit_and_unfinished_tail_is_not_consumed() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("executor.log");
        fs::write(
            &path,
            "media: no timestamp\n2026-10-23T04:00:00Z INFO later\n",
        )
        .unwrap();
        assert!(matches!(
            scan(
                root.path(),
                "pixels-file:executor.log",
                None,
                0,
                i64::MAX,
                1,
                4096
            ),
            Err(LogError::Unverified(_))
        ));
        let complete = "2026-10-23T04:00:00Z INFO ready\n";
        fs::write(&path, format!("{complete}2026-10-23T04:01")).unwrap();
        let page = scan(
            root.path(),
            "pixels-file:executor.log",
            None,
            0,
            i64::MAX,
            1,
            4096,
        )
        .unwrap();
        assert_eq!(page.events[0].raw, complete.as_bytes());
        let position = parse_position(page.next_native_position.as_deref().unwrap()).unwrap();
        assert_eq!(position.offset, complete.len() as u64);
    }
    #[test]
    fn text_after_preview_boundary_remains_available_for_exact_filtering() {
        let root = tempfile::tempdir().unwrap();
        let line = format!(
            "2026-10-13T01:00:00Z INFO {}tail-marker\n",
            "x".repeat(1500)
        );
        fs::write(root.path().join("executor.log"), line.as_bytes()).unwrap();
        let result = scan(
            root.path(),
            "pixels-file:executor.log",
            None,
            0,
            i64::MAX,
            1,
            4096,
        )
        .unwrap();
        assert_eq!(result.events.len(), 1);
        assert!(
            result.events[0]
                .message
                .as_deref()
                .unwrap()
                .contains("tail-marker")
        );
    }
    #[test]
    fn physical_generation_resume_and_rotation_gap() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("executor.log");
        fs::write(
            &path,
            b"2026-10-13T01:00:00Z INFO one\n2026-10-13T01:00:00Z INFO two\n",
        )
        .unwrap();
        let inventory = sources(root.path(), "test").unwrap();
        assert_eq!(inventory.len(), 1);
        let first = scan(
            root.path(),
            "pixels-file:executor.log",
            None,
            0,
            i64::MAX,
            1,
            1024,
        )
        .unwrap();
        assert_eq!(first.events.len(), 1);
        let second = scan(
            root.path(),
            "pixels-file:executor.log",
            first.next_native_position.as_deref(),
            0,
            i64::MAX,
            1,
            1024,
        )
        .unwrap();
        assert_eq!(second.events.len(), 1);
        fs::rename(&path, root.path().join("executor.log.1")).unwrap();
        fs::write(&path, b"2026-10-13T01:00:01Z INFO new\n").unwrap();
        assert!(matches!(
            scan(
                root.path(),
                "pixels-file:executor.log",
                second.next_native_position.as_deref(),
                0,
                i64::MAX,
                1,
                1024
            ),
            Err(LogError::Gap(_))
        ));
    }
}
