use crate::FileError;
use pab_protocol::TextEncoding;

pub fn tail_skip(bytes: &[u8], encoding: TextEncoding) -> usize {
    match encoding {
        TextEncoding::Utf8 | TextEncoding::Utf8Bom => bytes
            .iter()
            .take(3)
            .take_while(|b| **b & 0xc0 == 0x80)
            .count(),
        _ if bytes.len() >= 2 => {
            let unit = if encoding == TextEncoding::Utf16Le {
                u16::from_le_bytes([bytes[0], bytes[1]])
            } else {
                u16::from_be_bytes([bytes[0], bytes[1]])
            };
            if (0xdc00..=0xdfff).contains(&unit) {
                2
            } else {
                0
            }
        }
        _ => 0,
    }
}
// Leave an unfinished final code point for the next read. Invalid interior bytes are errors.
pub fn decode(bytes: &[u8], encoding: TextEncoding) -> Result<(String, usize, bool), FileError> {
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
    crate::validate_text(&decoded)?;
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
