//! Bounded SDK windows, shared Unicode decoding and authenticated caller-bound cursors.
use super::*;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    identity: String,
    offset: u64,
    encoding: TextEncoding,
    head: String,
    anchor: String,
}

fn unhex(value: &Value, maximum: usize) -> Result<Vec<u8>, FileError> {
    let text = value
        .as_str()
        .ok_or_else(|| FileError::new("invalid_result", "read", "Missing log window"))?;
    if text.len() > maximum * 2 || !text.len().is_multiple_of(2) {
        return Err(FileError::new(
            "invalid_result",
            "read",
            "Invalid log window size",
        ));
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let hex = std::str::from_utf8(pair)
                .map_err(|_| FileError::new("invalid_result", "read", "Invalid log hex"))?;
            u8::from_str_radix(hex, 16)
                .map_err(|_| FileError::new("invalid_result", "read", "Invalid log hex"))
        })
        .collect()
}
fn changed() -> FileError {
    FileError::new(
        "log_changed",
        "read",
        "File identity, length or retained bytes changed; start a fresh read",
    )
}

impl FileService {
    pub(super) async fn log_read(
        &self,
        owner: &str,
        request: &FileSystemRequest,
        reply: &mut FileSystemReply,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, FileError> {
        let FileSystemAction::Read {
            range:
                TextReadRange::Stream {
                    offset,
                    tail_bytes,
                    cursor,
                    max_bytes,
                    wait_ms,
                },
            encoding: explicit,
            ..
        } = &request.operation
        else {
            unreachable!()
        };
        let mut previous: Option<Cursor> = cursor
            .as_ref()
            .map(|token| {
                let payload = self.open_cursor(owner, &request.path, "log", token)?;
                serde_json::from_str(&payload)
                    .map_err(|_| FileError::new("invalid_cursor", "read", "Invalid log cursor"))
            })
            .transpose()?;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(*wait_ms as u64);
        loop {
            cancelled(cancel)?;
            let probe=self.io(json!({"action":"log_probe","path":request.path,"offset":previous.as_ref().map(|c|c.offset).or(*offset),"tail_bytes":tail_bytes,"max_bytes":max_bytes,"encoding":explicit})).await.map_err(|error| if previous.is_some() && matches!(error.0.code.as_str(),"invalid_range"|"not_found") {changed()} else {error})?;
            cancelled(cancel)?;
            let identity = probe["identity"]
                .as_str()
                .filter(|s| s.len() <= 128)
                .ok_or_else(changed)?;
            let raw = probe["start"].as_u64().ok_or_else(changed)?;
            let context = probe["context_start"].as_u64().ok_or_else(changed)?;
            let window = unhex(&probe["window"], 1024 + *max_bytes as usize)?;
            let head = unhex(&probe["head"], 1024)?;
            let length = probe["metadata"]["size"].as_u64().ok_or_else(changed)?;
            let slice = |start: u64, end: u64| -> Result<&[u8], FileError> {
                let start = usize::try_from(start.checked_sub(context).ok_or_else(changed)?)
                    .map_err(|_| changed())?;
                let end = usize::try_from(end.checked_sub(context).ok_or_else(changed)?)
                    .map_err(|_| changed())?;
                window.get(start..end).ok_or_else(changed)
            };
            let bom = if head.starts_with(&[0xef, 0xbb, 0xbf]) {
                3
            } else if head.starts_with(&[0xff, 0xfe]) || head.starts_with(&[0xfe, 0xff]) {
                2
            } else {
                0
            };
            let encoding = decode(&head[..bom], *explicit)?.encoding;
            if let Some(old) = &previous {
                let head_len = old.offset.min(1024) as usize;
                if old.identity != identity
                    || old.encoding != encoding
                    || length < old.offset
                    || head_len > head.len()
                    || digest(&head[..head_len]) != old.head
                    || digest(slice(old.offset.saturating_sub(1024), old.offset)?) != old.anchor
                {
                    return Err(changed());
                }
            }
            let mut start = raw.max(bom as u64);
            let tail = previous.is_none() && tail_bytes.is_some();
            if matches!(encoding, TextEncoding::Utf16Le | TextEncoding::Utf16Be)
                && !(start - bom as u64).is_multiple_of(2)
            {
                if tail {
                    start += 1;
                } else {
                    return Err(FileError::new(
                        "invalid_range",
                        "read",
                        "UTF-16 offset must be a code-unit boundary",
                    ));
                }
            }
            let end = context + window.len() as u64;
            if start > end || end > raw.saturating_add(*max_bytes as u64) {
                return Err(changed());
            }
            if tail {
                start += pab_file_codec::log::tail_skip(slice(start, end)?, encoding) as u64;
            }
            let (text, consumed, incomplete) =
                pab_file_codec::log::decode(slice(start, end)?, encoding)?;
            let next = start + consumed as u64;
            let head_len = next.min(1024) as usize;
            if head_len > head.len() {
                return Err(changed());
            }
            let next_cursor = Cursor {
                identity: identity.into(),
                offset: next,
                encoding,
                head: digest(&head[..head_len]),
                anchor: digest(slice(next.saturating_sub(1024), next)?),
            };
            if consumed > 0 || tokio::time::Instant::now() >= deadline {
                let mut meta = metadata(&probe["metadata"]);
                meta.encoding = Some(encoding);
                reply.metadata = Some(meta);
                reply.range = Some(TextReadPosition {
                    offset: start,
                    next_offset: next,
                    next_line: None,
                    truncated: next < length,
                });
                reply.log = Some(LogReadState {
                    cursor: self.seal_cursor(
                        owner,
                        &request.path,
                        "log",
                        &serde_json::to_string(&next_cursor).map_err(|_| changed())?,
                    )?,
                    wait_expired: consumed == 0 && *wait_ms > 0,
                    incomplete_character: incomplete,
                });
                return Ok(text.into_bytes());
            }
            previous = Some(next_cursor);
            tokio::time::sleep_until(
                deadline.min(tokio::time::Instant::now() + Duration::from_millis(100)),
            )
            .await;
        }
    }
}
