use super::{filesystem::FileError, filesystem_io as io, filesystem_text as text};
use pab_protocol::{FileSystemReply, LogReadState, TextEncoding, TextReadPosition, TextReadRange};
use serde::{Deserialize, Serialize};
use std::{
    hash::{Hash, Hasher},
    path::Path,
    time::Duration,
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    path: String,
    identity: u64,
    offset: u64,
    encoding: TextEncoding,
    head_len: u32,
    head: String,
    anchor: String,
}

async fn slice(file: &mut fs::File, offset: u64, count: usize) -> Result<Vec<u8>, FileError> {
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| io::io_error("seek", e))?;
    let mut bytes = Vec::with_capacity(count);
    (&mut *file)
        .take(count as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| io::io_error("read", e))?;
    Ok(bytes)
}
async fn identity(file: &fs::File) -> Result<u64, FileError> {
    let file = file
        .try_clone()
        .await
        .map_err(|e| io::io_error("identity", e))?
        .into_std()
        .await;
    tokio::task::spawn_blocking(move || {
        let handle = same_file::Handle::from_file(file).map_err(|e| io::io_error("identity", e))?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        handle.hash(&mut h);
        Ok(h.finish())
    })
    .await
    .map_err(|e| FileError::new("identity_failed", "read", e.to_string()))?
}
fn conflict() -> FileError {
    FileError::new(
        "log_changed",
        "read",
        "file was replaced, truncated, or retained bytes changed; start a fresh tail/read",
    )
}
async fn cursor(
    file: &mut fs::File,
    path: String,
    id: u64,
    offset: u64,
    encoding: TextEncoding,
) -> Result<Cursor, FileError> {
    let head_len = offset.min(1024) as u32;
    Ok(Cursor {
        version: 1,
        path,
        identity: id,
        offset,
        encoding,
        head_len,
        head: io::digest(&slice(file, 0, head_len as usize).await?),
        anchor: io::digest(
            &slice(file, offset.saturating_sub(1024), offset.min(1024) as usize).await?,
        ),
    })
}
async fn verify(
    file: &mut fs::File,
    c: &Cursor,
    path: &str,
    id: u64,
    length: u64,
) -> Result<(), FileError> {
    if c.version != 1
        || c.path != path
        || c.identity != id
        || length < c.offset
        || c.head_len != c.offset.min(1024) as u32
        || io::digest(&slice(file, 0, c.head_len as usize).await?) != c.head
        || io::digest(
            &slice(
                file,
                c.offset.saturating_sub(1024),
                c.offset.min(1024) as usize,
            )
            .await?,
        ) != c.anchor
    {
        return Err(conflict());
    }
    Ok(())
}
// Leave an unfinished final code point for the next read. Invalid interior bytes are errors.
fn decode(bytes: &[u8], encoding: TextEncoding) -> Result<(String, usize, bool), FileError> {
    let invalid = || {
        FileError::new(
            "invalid_encoding",
            "decode",
            "invalid encoded log text; no lossy replacement performed",
        )
    };
    let (decoded, available) = match encoding {
        TextEncoding::Utf8 | TextEncoding::Utf8Bom => {
            let end = match std::str::from_utf8(bytes) {
                Ok(_) => bytes.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(_) => return Err(invalid()),
            };
            (
                std::str::from_utf8(&bytes[..end])
                    .map_err(|_| invalid())?
                    .to_owned(),
                end,
            )
        }
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            let mut units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|p| {
                    if encoding == TextEncoding::Utf16Le {
                        u16::from_le_bytes([p[0], p[1]])
                    } else {
                        u16::from_be_bytes([p[0], p[1]])
                    }
                })
                .collect();
            if units.last().is_some_and(|u| (0xd800..=0xdbff).contains(u)) {
                units.pop();
            }
            let length = units.len() * 2;
            (String::from_utf16(&units).map_err(|_| invalid())?, length)
        }
    };
    text::validate_text(&decoded)?;
    let mut result = String::new();
    let mut consumed = 0;
    for ch in decoded.chars() {
        if result.len() + ch.len_utf8() > 16384 {
            break;
        }
        result.push(ch);
        consumed += match encoding {
            TextEncoding::Utf8 | TextEncoding::Utf8Bom => ch.len_utf8(),
            _ => ch.len_utf16() * 2,
        };
    }
    Ok((result, consumed, available < bytes.len()))
}

pub(super) async fn read(
    path: &Path,
    range: &TextReadRange,
    explicit: Option<TextEncoding>,
    reply: &mut FileSystemReply,
) -> Result<Vec<u8>, FileError> {
    let TextReadRange::Stream {
        offset,
        tail_bytes,
        cursor: token,
        max_bytes,
        wait_ms,
    } = range
    else {
        unreachable!()
    };
    let mut previous: Option<Cursor> = token
        .as_ref()
        .map(|s| {
            serde_json::from_str(s)
                .map_err(|_| FileError::new("invalid_cursor", "validate", "invalid log cursor"))
        })
        .transpose()?;
    let path_hash = io::digest(path.to_string_lossy().as_bytes());
    let deadline = tokio::time::Instant::now() + Duration::from_millis(*wait_ms as u64);
    loop {
        io::no_links(path, false).await?;
        let named = fs::metadata(path)
            .await
            .map_err(|e| io::io_error("stat", e))?;
        if !named.is_file() {
            return Err(FileError::new(
                "not_regular_file",
                "read",
                "log must be an ordinary file",
            ));
        }
        let mut file = fs::File::open(path)
            .await
            .map_err(|e| io::io_error("open", e))?;
        let id = identity(&file).await?;
        let before = file.metadata().await.map_err(|e| io::io_error("stat", e))?;
        if !before.is_file() {
            return Err(FileError::new(
                "not_regular_file",
                "read",
                "log must be an ordinary file",
            ));
        }
        let header = slice(&mut file, 0, 3).await?;
        let bom = if header.starts_with(&[0xef, 0xbb, 0xbf]) {
            3
        } else if header.starts_with(&[0xff, 0xfe]) || header.starts_with(&[0xfe, 0xff]) {
            2
        } else {
            0
        };
        let encoding = text::decode(&header[..bom], explicit)?.encoding;
        let mut start = if let Some(c) = &previous {
            verify(&mut file, c, &path_hash, id, before.len()).await?;
            if c.encoding != encoding {
                return Err(conflict());
            }
            c.offset
        } else {
            offset.unwrap_or_else(|| {
                tail_bytes
                    .map(|n| before.len().saturating_sub(n as u64))
                    .unwrap_or(0)
            })
        };
        start = start.max(bom as u64);
        if start > before.len() {
            return Err(FileError::new(
                "invalid_range",
                "read",
                "offset is past EOF",
            ));
        }
        let tail = previous.is_none() && tail_bytes.is_some();
        if !tail
            && matches!(encoding, TextEncoding::Utf16Le | TextEncoding::Utf16Be)
            && !(start - bom as u64).is_multiple_of(2)
        {
            return Err(FileError::new(
                "invalid_range",
                "read",
                "UTF-16 offset must be a code-unit boundary",
            ));
        }
        if tail
            && matches!(encoding, TextEncoding::Utf16Le | TextEncoding::Utf16Be)
            && !(start - bom as u64).is_multiple_of(2)
        {
            start += 1;
        }
        let mut bytes = slice(&mut file, start, *max_bytes as usize).await?;
        if tail {
            let skip = match encoding {
                TextEncoding::Utf8 | TextEncoding::Utf8Bom => bytes
                    .iter()
                    .take(3)
                    .take_while(|b| **b & 0xc0 == 0x80)
                    .count(),
                _ if bytes.len() >= 2 => {
                    let u = if encoding == TextEncoding::Utf16Le {
                        u16::from_le_bytes([bytes[0], bytes[1]])
                    } else {
                        u16::from_be_bytes([bytes[0], bytes[1]])
                    };
                    if (0xdc00..=0xdfff).contains(&u) { 2 } else { 0 }
                }
                _ => 0,
            };
            start += skip as u64;
            bytes.drain(..skip);
        }
        let (decoded, consumed, incomplete) = decode(&bytes, encoding)?;
        let after = file.metadata().await.map_err(|e| io::io_error("stat", e))?;
        let named_file = fs::File::open(path)
            .await
            .map_err(|e| io::io_error("observe", e))?;
        if identity(&named_file).await? != id
            || after.len() < before.len()
            || (after.len() == before.len() && after.modified().ok() != before.modified().ok())
        {
            return Err(conflict());
        }
        if let Some(c) = &previous {
            verify(&mut file, c, &path_hash, id, after.len()).await?;
        }
        let next = cursor(
            &mut file,
            path_hash.clone(),
            id,
            start + consumed as u64,
            encoding,
        )
        .await?;
        // Re-read the consumed bytes so an observed rewrite is never accepted as an append.
        if slice(&mut file, start, consumed).await? != bytes[..consumed] {
            return Err(conflict());
        }
        let expired = tokio::time::Instant::now() >= deadline;
        if consumed > 0 || expired {
            let mut meta = io::metadata(&after);
            meta.encoding = Some(encoding);
            reply.metadata = Some(meta);
            reply.range = Some(TextReadPosition {
                offset: start,
                next_offset: next.offset,
                next_line: None,
                truncated: next.offset < after.len(),
            });
            reply.log = Some(LogReadState {
                cursor: serde_json::to_string(&next)
                    .map_err(|e| FileError::new("invalid_cursor", "encode", e.to_string()))?,
                wait_expired: consumed == 0 && *wait_ms > 0,
                incomplete_character: incomplete,
            });
            let data = decoded.into_bytes();
            reply.data_size = data.len() as u32;
            reply.data_sha256 = Some(io::digest(&data));
            return Ok(data);
        }
        previous = Some(next);
        drop(file);
        drop(named_file);
        tokio::time::sleep_until(
            deadline.min(tokio::time::Instant::now() + Duration::from_millis(100)),
        )
        .await;
    }
}
