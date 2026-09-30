use pab_protocol::{
    MAX_TEXT_EDITS, MAX_TEXT_FILE_BYTES, MAX_TEXT_READ_BYTES, TextEdit, TextEncoding,
    TextReadPosition, TextReadRange,
};

use super::filesystem::FileError;

pub(super) struct Document {
    pub text: String,
    pub encoding: TextEncoding,
    pub bom: usize,
}

pub(super) fn decode(bytes: &[u8], explicit: Option<TextEncoding>) -> Result<Document, FileError> {
    let detected = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        Some((TextEncoding::Utf8Bom, 3))
    } else if bytes.starts_with(&[0xff, 0xfe]) {
        Some((TextEncoding::Utf16Le, 2))
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        Some((TextEncoding::Utf16Be, 2))
    } else {
        None
    };
    let (encoding, bom) = match (detected, explicit) {
        (Some((actual, bom)), Some(wanted))
            if actual == wanted
                || (actual == TextEncoding::Utf8Bom && wanted == TextEncoding::Utf8) =>
        {
            (actual, bom)
        }
        (Some(_), Some(_)) => {
            return Err(FileError::new(
                "encoding_conflict",
                "decode",
                "requested encoding conflicts with the file BOM",
            ));
        }
        (Some(actual), None) => actual,
        (None, Some(TextEncoding::Utf8Bom)) => {
            return Err(FileError::new(
                "encoding_conflict",
                "decode",
                "UTF-8 BOM was requested but is absent",
            ));
        }
        (None, encoding) => (encoding.unwrap_or(TextEncoding::Utf8), 0),
    };
    let data = &bytes[bom..];
    let text = match encoding {
        TextEncoding::Utf8 | TextEncoding::Utf8Bom => String::from_utf8(data.to_vec()).map_err(|_| FileError::new("invalid_encoding", "decode", "file is not valid UTF-8; specify a supported encoding or download it as binary"))?,
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            if !data.len().is_multiple_of(2) { return Err(FileError::new("invalid_encoding", "decode", "UTF-16 has an incomplete code unit")); }
            let units = data.chunks_exact(2).map(|pair| if encoding == TextEncoding::Utf16Le { u16::from_le_bytes([pair[0], pair[1]]) } else { u16::from_be_bytes([pair[0], pair[1]]) }).collect::<Vec<_>>();
            String::from_utf16(&units).map_err(|_| FileError::new("invalid_encoding", "decode", "UTF-16 contains an unpaired surrogate"))?
        }
    };
    validate_text(&text)?;
    Ok(Document {
        text,
        encoding,
        bom,
    })
}

pub(super) fn validate_text(text: &str) -> Result<(), FileError> {
    if text
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t' | '\u{c}'))
    {
        return Err(FileError::new(
            "not_text",
            "decode",
            "binary/control bytes are not supported by text tools; use file transfer",
        ));
    }
    Ok(())
}

pub(super) fn encode(text: &str, encoding: TextEncoding, bom: bool) -> Result<Vec<u8>, FileError> {
    validate_text(text)?;
    let mut bytes = Vec::new();
    match encoding {
        TextEncoding::Utf8 => bytes.extend_from_slice(text.as_bytes()),
        TextEncoding::Utf8Bom => {
            bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
            bytes.extend_from_slice(text.as_bytes());
        }
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            if bom {
                bytes.extend_from_slice(if encoding == TextEncoding::Utf16Le {
                    &[0xff, 0xfe]
                } else {
                    &[0xfe, 0xff]
                });
            }
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&if encoding == TextEncoding::Utf16Le {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
        }
    }
    if bytes.len() > MAX_TEXT_FILE_BYTES {
        return Err(FileError::new(
            "file_too_large",
            "encode",
            "text result exceeds 4 MiB; use file transfer",
        ));
    }
    Ok(bytes)
}

pub(super) fn newline(text: &str) -> String {
    let crlf = text.matches("\r\n").count();
    let lf = text.bytes().filter(|b| *b == b'\n').count() - crlf;
    let cr = text.bytes().filter(|b| *b == b'\r').count() - crlf;
    match (crlf > 0, lf > 0, cr > 0) {
        (false, false, false) => "none",
        (true, false, false) => "crlf",
        (false, true, false) => "lf",
        (false, false, true) => "cr",
        _ => "mixed",
    }
    .to_owned()
}

fn raw_len(text: &str, encoding: TextEncoding) -> usize {
    match encoding {
        TextEncoding::Utf8 | TextEncoding::Utf8Bom => text.len(),
        _ => text.encode_utf16().count() * 2,
    }
}

pub(super) fn read(
    doc: &Document,
    range: &TextReadRange,
) -> Result<(Vec<u8>, TextReadPosition), FileError> {
    let text = doc.text.as_str();
    let (start, wanted_end, raw_budget, next_line) = match range {
        TextReadRange::Bytes { offset, max_bytes } => {
            let offset = usize::try_from(*offset)
                .map_err(|_| FileError::new("invalid_range", "read", "byte offset is too large"))?;
            let offset = if offset == 0 { doc.bom } else { offset };
            let mut raw = doc.bom;
            let mut start = None;
            for (index, ch) in text.char_indices() {
                if raw == offset {
                    start = Some(index);
                    break;
                }
                raw += match doc.encoding {
                    TextEncoding::Utf8 | TextEncoding::Utf8Bom => ch.len_utf8(),
                    _ => ch.len_utf16() * 2,
                };
            }
            if start.is_none() && raw == offset {
                start = Some(text.len());
            }
            let start = start.ok_or_else(|| {
                FileError::new(
                    "invalid_range",
                    "read",
                    "offset is past EOF, inside the BOM, or inside an encoded character",
                )
            })?;
            (start, text.len(), *max_bytes as usize, None)
        }
        TextReadRange::Lines { start_line, count } => {
            let mut line = 1_u64;
            let mut start = (*start_line == 1).then_some(0);
            let mut end = None;
            for (index, ch) in text.char_indices() {
                if ch == '\n' || (ch == '\r' && !text[index + 1..].starts_with('\n')) {
                    line += 1;
                    let boundary = index + ch.len_utf8();
                    if line == *start_line {
                        start = Some(boundary);
                    }
                    if line == start_line.saturating_add(u64::from(*count)) {
                        end = Some(boundary);
                        break;
                    }
                }
            }
            let start = start.unwrap_or(text.len());
            let end = end.unwrap_or(text.len());
            (
                start,
                end,
                usize::MAX,
                (end < text.len()).then_some(start_line.saturating_add(u64::from(*count))),
            )
        }
    };
    let mut end = start;
    let mut raw = 0;
    for ch in text[start..wanted_end].chars() {
        let raw_size = match doc.encoding {
            TextEncoding::Utf8 | TextEncoding::Utf8Bom => ch.len_utf8(),
            _ => ch.len_utf16() * 2,
        };
        if end - start + ch.len_utf8() > MAX_TEXT_READ_BYTES || raw + raw_size > raw_budget {
            break;
        }
        end += ch.len_utf8();
        raw += raw_size;
    }
    let offset = doc.bom + raw_len(&text[..start], doc.encoding);
    Ok((
        text.as_bytes()[start..end].to_vec(),
        TextReadPosition {
            offset: offset as u64,
            next_offset: (offset + raw) as u64,
            next_line: if end == wanted_end { next_line } else { None },
            truncated: end < text.len(),
        },
    ))
}

pub(super) fn patch(text: &str, edits: &[TextEdit]) -> Result<String, FileError> {
    if edits.is_empty() || edits.len() > MAX_TEXT_EDITS {
        return Err(FileError::new(
            "invalid_edits",
            "patch",
            "provide 1..32 exact replacements",
        ));
    }
    let mut spans = Vec::new();
    let mut output_size = text.len();
    for edit in edits {
        if edit.find.is_empty() || !(1..=1000).contains(&edit.expected_matches) {
            return Err(FileError::new(
                "invalid_edits",
                "patch",
                "find must be nonempty; expected_matches must be 1..1000",
            ));
        }
        validate_text(&edit.replace)?;
        let matches = text
            .match_indices(&edit.find)
            .take(edit.expected_matches as usize + 1)
            .collect::<Vec<_>>();
        if matches.len() != edit.expected_matches as usize {
            return Err(FileError::new(
                "match_conflict",
                "patch",
                format!(
                    "expected {} matches, found {}{}",
                    edit.expected_matches,
                    matches.len(),
                    if matches.len() > edit.expected_matches as usize {
                        " or more"
                    } else {
                        ""
                    }
                ),
            ));
        }
        for (offset, _) in matches {
            output_size = output_size
                .checked_sub(edit.find.len())
                .and_then(|size| size.checked_add(edit.replace.len()))
                .filter(|size| *size <= MAX_TEXT_FILE_BYTES * 3 / 2)
                .ok_or_else(|| {
                    FileError::new("file_too_large", "patch", "decoded patch exceeds its memory budget; encoded files are limited to 4 MiB")
                })?;
            spans.push((offset, offset + edit.find.len(), edit.replace.as_str()));
        }
    }
    spans.sort_by_key(|(start, end, _)| (*start, *end));
    let mut result = String::with_capacity(output_size);
    let mut position = 0;
    for (start, end, replacement) in spans {
        if start < position {
            return Err(FileError::new(
                "overlapping_edits",
                "patch",
                "replacements overlap in the original text",
            ));
        }
        result.push_str(&text[position..start]);
        result.push_str(replacement);
        position = end;
    }
    result.push_str(&text[position..]);
    Ok(result)
}
