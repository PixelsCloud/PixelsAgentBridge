use std::ptr;

use chrono::{DateTime, SecondsFormat, Utc};
use pab_protocol::SystemLogSource;
use quick_xml::{events::Event, Reader};
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS},
    System::EventLog::{
        EvtChannelConfigEnabled, EvtClose, EvtCreateBookmark, EvtGetChannelConfigProperty, EvtNext,
        EvtNextChannelPath, EvtOpenChannelConfig, EvtOpenChannelEnum, EvtQuery,
        EvtQueryChannelPath, EvtQueryForwardDirection, EvtRender, EvtRenderBookmark,
        EvtRenderEventXml, EvtSeek, EvtSeekRelativeToBookmark, EvtSeekStrict, EvtUpdateBookmark,
        EvtVarTypeBoolean, EVT_HANDLE, EVT_VARIANT,
    },
};

use crate::{LogError, NativePage, NativeRecord};

const PREFIX: &str = "eventlog:";
const MAX_NATIVE_RECORD_BYTES: usize = 16 * 1024 * 1024;
const MAX_SCANNED_PER_CALL: usize = 10_000;

struct Handle(EVT_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { EvtClose(self.0) };
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error(operation: &str) -> LogError {
    let code = unsafe { GetLastError() };
    if code == 5 {
        LogError::PermissionDenied(format!("{operation}: Windows error {code}"))
    } else {
        LogError::Native(format!("{operation}: Windows error {code}"))
    }
}

fn channel_enabled(path: &[u16]) -> Result<bool, LogError> {
    let config = Handle(unsafe { EvtOpenChannelConfig(0, path.as_ptr(), 0) });
    if config.0 == 0 {
        return Err(last_error("EvtOpenChannelConfig"));
    }
    let mut property = EVT_VARIANT::default();
    let mut used = 0;
    if unsafe {
        EvtGetChannelConfigProperty(
            config.0,
            EvtChannelConfigEnabled,
            0,
            std::mem::size_of::<EVT_VARIANT>() as u32,
            &mut property,
            &mut used,
        )
    } == 0
    {
        return Err(last_error("EvtGetChannelConfigProperty"));
    }
    if property.Type != EvtVarTypeBoolean as u32 {
        return Err(LogError::Native(
            "unexpected Event Log channel enabled type".into(),
        ));
    }
    Ok(unsafe { property.Anonymous.BooleanVal != 0 })
}

fn render(handle: EVT_HANDLE, flags: u32) -> Result<String, LogError> {
    let mut used = 0;
    let mut properties = 0;
    unsafe {
        if EvtRender(
            0,
            handle,
            flags,
            0,
            ptr::null_mut(),
            &mut used,
            &mut properties,
        ) != 0
        {
            return Err(LogError::Native(
                "EvtRender returned an empty buffer".into(),
            ));
        }
        if GetLastError() != ERROR_INSUFFICIENT_BUFFER {
            return Err(last_error("EvtRender(size)"));
        }
    }
    if used as usize > MAX_NATIVE_RECORD_BYTES * 2 || used % 2 != 0 {
        return Err(LogError::Native("event XML exceeds 16 MiB budget".into()));
    }
    let mut buffer = vec![0_u16; (used as usize / 2).max(1)];
    if unsafe {
        EvtRender(
            0,
            handle,
            flags,
            used,
            buffer.as_mut_ptr().cast(),
            &mut used,
            &mut properties,
        )
    } == 0
    {
        return Err(last_error("EvtRender"));
    }
    let len = used as usize / 2;
    if len > buffer.len() {
        return Err(LogError::Native("EvtRender length exceeded buffer".into()));
    }
    let without_nul = buffer[..len]
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(len);
    String::from_utf16(&buffer[..without_nul])
        .map_err(|_| LogError::Native("event XML contains invalid UTF-16".into()))
}

fn metadata(xml: &str) -> Result<(i64, Option<String>), LogError> {
    let mut reader = Reader::from_str(xml);
    let mut timestamp = None;
    let mut level = None;
    let mut in_level = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) | Ok(Event::Empty(start)) => {
                if start.local_name().as_ref() == b"TimeCreated" {
                    for attribute in start.attributes().flatten() {
                        if attribute.key.as_ref() == b"SystemTime" {
                            let value = std::str::from_utf8(attribute.value.as_ref())
                                .map_err(|_| LogError::Native("invalid event time".into()))?;
                            timestamp = DateTime::parse_from_rfc3339(value)
                                .ok()
                                .map(|time| time.timestamp_millis());
                        }
                    }
                }
                if start.local_name().as_ref() == b"Level" {
                    in_level = true;
                }
            }
            Ok(Event::Text(text)) if in_level => {
                level = std::str::from_utf8(text.as_ref()).ok().map(str::to_owned);
                in_level = false;
            }
            Ok(Event::End(end)) if end.local_name().as_ref() == b"Level" => {
                in_level = false;
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(LogError::Native(format!("invalid event XML: {error}"))),
            _ => {}
        }
    }
    timestamp
        .map(|time| (time, level))
        .ok_or_else(|| LogError::Native("event lacks a parseable TimeCreated".into()))
}

pub(super) fn sources(identity: &str) -> Result<Vec<SystemLogSource>, LogError> {
    let enumeration = Handle(unsafe { EvtOpenChannelEnum(0, 0) });
    if enumeration.0 == 0 {
        return Err(last_error("EvtOpenChannelEnum"));
    }
    let mut sources = Vec::new();
    loop {
        let mut used = 0;
        let initial = unsafe { EvtNextChannelPath(enumeration.0, 0, ptr::null_mut(), &mut used) };
        if initial != 0 {
            return Err(LogError::Native(
                "EvtNextChannelPath returned no path".into(),
            ));
        }
        let error = unsafe { GetLastError() };
        if error == ERROR_NO_MORE_ITEMS {
            break;
        }
        if error != ERROR_INSUFFICIENT_BUFFER || used == 0 || used > 32_768 {
            return Err(last_error("EvtNextChannelPath(size)"));
        }
        let mut buffer = vec![0_u16; used as usize];
        if unsafe { EvtNextChannelPath(enumeration.0, used, buffer.as_mut_ptr(), &mut used) } == 0 {
            return Err(last_error("EvtNextChannelPath"));
        }
        let len = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        let channel = String::from_utf16(&buffer[..len])
            .map_err(|_| LogError::Native("invalid channel path UTF-16".into()))?;
        let channel_wide = wide(&channel);
        let (availability, reason) = match channel_enabled(&channel_wide) {
            Ok(false) => ("disabled", Some("Event Log channel is disabled".into())),
            Ok(true) => {
                let query = Handle(unsafe {
                    EvtQuery(
                        0,
                        channel_wide.as_ptr(),
                        ptr::null(),
                        EvtQueryChannelPath | EvtQueryForwardDirection,
                    )
                });
                if query.0 == 0 {
                    let error = last_error("EvtQuery");
                    (
                        if matches!(error, LogError::PermissionDenied(_)) {
                            "permission_denied"
                        } else {
                            "unavailable"
                        },
                        Some(error.to_string()),
                    )
                } else {
                    ("readable", None)
                }
            }
            Err(error) => (
                if matches!(error, LogError::PermissionDenied(_)) {
                    "permission_denied"
                } else {
                    "unavailable"
                },
                Some(error.to_string()),
            ),
        };
        sources.push(SystemLogSource {
            source_id: format!("{PREFIX}{channel}"),
            source_kind: "windows_event_log".into(),
            source_locator: channel,
            availability: availability.into(),
            reason,
            execution_identity: identity.into(),
            coverage_limit: Some("Disabled channels are not enabled automatically; ETW traces and arbitrary text logs are separate sources".into()),
        });
    }
    Ok(sources)
}

pub(super) fn scan(
    source: &str,
    after: Option<&str>,
    start_unix_ms: i64,
    end_unix_ms: i64,
    max_records: usize,
    max_bytes: usize,
) -> Result<NativePage, LogError> {
    let channel = source
        .strip_prefix(PREFIX)
        .filter(|name| !name.is_empty() && !name.contains('\0'))
        .ok_or_else(|| LogError::Unavailable("invalid Event Log channel".into()))?;
    if max_records == 0 || max_records > 128 || max_bytes == 0 {
        return Err(LogError::Native("invalid Event Log page budget".into()));
    }
    let channel_wide = wide(channel);
    // The time predicate is an optimization only. We still validate each
    // rendered native timestamp and carry the Event Log bookmark across calls.
    let expression = DateTime::<Utc>::from_timestamp_millis(start_unix_ms).map(|start| {
        format!(
            "*[System[TimeCreated[@SystemTime >= '{}']]]",
            start.to_rfc3339_opts(SecondsFormat::Millis, true)
        )
    });
    let expression_wide = expression.as_deref().map(wide);
    let query = Handle(unsafe {
        EvtQuery(
            0,
            channel_wide.as_ptr(),
            expression_wide
                .as_ref()
                .map_or(ptr::null(), |value| value.as_ptr()),
            EvtQueryChannelPath | EvtQueryForwardDirection,
        )
    });
    if query.0 == 0 {
        return Err(last_error("EvtQuery"));
    }
    if let Some(previous) = after {
        let previous_wide = wide(previous);
        let bookmark = Handle(unsafe { EvtCreateBookmark(previous_wide.as_ptr()) });
        if bookmark.0 == 0 {
            return Err(LogError::Gap(
                "Event Log bookmark cannot be reopened".into(),
            ));
        }
        if unsafe {
            EvtSeek(
                query.0,
                0,
                bookmark.0,
                0,
                EvtSeekRelativeToBookmark | EvtSeekStrict,
            )
        } == 0
        {
            return Err(LogError::Gap(format!(
                "Event Log bookmark lost: {}",
                last_error("EvtSeek")
            )));
        }
        // Seeking one past a valid final record fails with ERROR_NOT_FOUND
        // under EvtSeekStrict. Seek to the bookmarked record instead, verify
        // it still exists in this query, then consume it exactly once.
        let mut event_handle: EVT_HANDLE = 0;
        let mut count = 0;
        if unsafe { EvtNext(query.0, 1, &mut event_handle, 0, 0, &mut count) } == 0
            || count != 1
            || event_handle == 0
        {
            return Err(LogError::Gap(
                "Event Log bookmark record disappeared".into(),
            ));
        }
        let event = Handle(event_handle);
        if unsafe { EvtUpdateBookmark(bookmark.0, event.0) } == 0
            || render(bookmark.0, EvtRenderBookmark)? != previous
        {
            return Err(LogError::Gap(
                "Event Log bookmark now points to a different record".into(),
            ));
        }
    }
    let bookmark = Handle(unsafe { EvtCreateBookmark(ptr::null()) });
    if bookmark.0 == 0 {
        return Err(last_error("EvtCreateBookmark"));
    }
    let mut page = NativePage {
        events: Vec::new(),
        next_native_position: after.map(str::to_owned),
        scanned_count: 0,
        exhausted: false,
        // EvtQuery includes newly written events while enumerating; without
        // a pinned native high-water mark, exhaustion is not a proof that no
        // late event with a clock-rolled-back timestamp can appear.
        verifiable: false,
    };
    let mut bytes = 0_usize;
    loop {
        let mut event_handle: EVT_HANDLE = 0;
        let mut count = 0;
        if unsafe { EvtNext(query.0, 1, &mut event_handle, 0, 0, &mut count) } == 0 {
            let error = unsafe { GetLastError() };
            if error == ERROR_NO_MORE_ITEMS {
                page.exhausted = true;
                break;
            }
            return Err(last_error("EvtNext"));
        }
        if count != 1 || event_handle == 0 {
            return Err(LogError::Native("EvtNext returned no event".into()));
        }
        let event = Handle(event_handle);
        let xml = render(event.0, EvtRenderEventXml)?;
        let (timestamp, level) = metadata(&xml)?;
        if unsafe { EvtUpdateBookmark(bookmark.0, event.0) } == 0 {
            return Err(last_error("EvtUpdateBookmark"));
        }
        let position = render(bookmark.0, EvtRenderBookmark)?;
        if timestamp >= start_unix_ms && timestamp < end_unix_ms {
            if !page.events.is_empty()
                && (page.events.len() >= max_records || bytes.saturating_add(xml.len()) > max_bytes)
            {
                break;
            }
            bytes += xml.len();
            page.events.push(NativeRecord {
                source_id: source.into(),
                source_kind: "windows_event_log".into(),
                source_locator: channel.into(),
                native_position: position.clone(),
                timestamp_unix_ms: timestamp,
                native_level: level,
                message: Some(xml.clone()),
                raw: xml.into_bytes(),
            });
        }
        page.next_native_position = Some(position);
        page.scanned_count += 1;
        if page.scanned_count as usize >= MAX_SCANNED_PER_CALL {
            break;
        }
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_native_channels_and_scans_application() {
        let inventory = sources("test").expect("enumerate Event Log channels");
        assert!(inventory
            .iter()
            .any(|entry| entry.source_id == "eventlog:Application"));
        let end = chrono::Utc::now().timestamp_millis();
        let page = scan(
            "eventlog:Application",
            None,
            end - 3_600_000,
            end,
            4,
            65_536,
        )
        .expect("read Application events");
        assert!(page.events.len() <= 4);
        assert!(page
            .events
            .iter()
            .all(|event| event.source_id == "eventlog:Application"));
        if let Some(cursor) = page.next_native_position {
            let resumed = scan(
                "eventlog:Application",
                Some(&cursor),
                end - 3_600_000,
                end,
                4,
                65_536,
            )
            .expect("resume from a native bookmark");
            if let (Some(left), Some(right)) = (page.events.last(), resumed.events.first()) {
                assert_ne!(left.native_position, right.native_position);
            }
        }
    }

    #[test]
    fn bookmark_resumes_before_tail_when_application_has_multiple_records() {
        let end = chrono::Utc::now().timestamp_millis();
        let start = end - 7 * 24 * 3_600_000;
        let first = scan("eventlog:Application", None, start, end, 1, 65_536).unwrap();
        if !first.exhausted {
            let cursor = first.next_native_position.as_deref().unwrap();
            let second = scan("eventlog:Application", Some(cursor), start, end, 1, 65_536).unwrap();
            if let Some(event) = second.events.first() {
                assert_ne!(first.events[0].native_position, event.native_position);
            }
        }
    }
}
