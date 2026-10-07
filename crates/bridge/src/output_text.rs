//! Display decoding only. Persisted task output and all offsets remain raw bytes.
use encoding_rs::{BIG5, CoderResult, GB18030, GBK, UTF_8, UTF_16BE, UTF_16LE};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputEncoding {
    #[default]
    Utf8,
    Gbk,
    Gb18030,
    Big5,
    Utf16Le,
    Utf16Be,
}

impl OutputEncoding {
    pub fn validate_offset(self, offset: u64) -> Result<(), &'static str> {
        if matches!(self, Self::Utf16Le | Self::Utf16Be) && offset % 2 != 0 {
            return Err(
                "UTF-16 output requires an even byte offset; re-read from an aligned offset",
            );
        }
        Ok(())
    }

    /// Tail selection may land between UTF-16 code units. Skip the partial unit;
    /// arbitrary multibyte starting points are still explicitly unverified.
    pub fn align_start(self, offset: u64) -> u64 {
        if matches!(self, Self::Utf16Le | Self::Utf16Be) && offset % 2 != 0 {
            offset.saturating_add(1)
        } else {
            offset
        }
    }
}

#[derive(Debug)]
pub struct DecodedOutput {
    pub text: String,
    pub consumed: usize,
    pub pending_bytes: usize,
    pub replacements: bool,
}

/// Decode a bounded slice. `eof` means the slice reaches the COMPLETE stream's
/// end, not merely that this read exhausted its byte budget. A non-final partial
/// character stays behind the returned byte cursor and can be re-read statelessly.
pub fn decode_output(bytes: &[u8], encoding: OutputEncoding, eof: bool) -> DecodedOutput {
    let codec = match encoding {
        OutputEncoding::Utf8 => UTF_8,
        OutputEncoding::Gbk => GBK,
        OutputEncoding::Gb18030 => GB18030,
        OutputEncoding::Big5 => BIG5,
        OutputEncoding::Utf16Le => UTF_16LE,
        OutputEncoding::Utf16Be => UTF_16BE,
    };
    if eof {
        let (text, replacements) = codec.decode_without_bom_handling(bytes);
        return DecodedOutput {
            text: text.into_owned(),
            consumed: bytes.len(),
            pending_bytes: 0,
            replacements,
        };
    }
    let mut decoder = codec.new_decoder_without_bom_handling();
    let mut text = String::with_capacity(
        decoder
            .max_utf8_buffer_length(bytes.len())
            .expect("bounded output read"),
    );
    let (status, read, replacements) = decoder.decode_to_string(bytes, &mut text, false);
    assert_eq!(status, CoderResult::InputEmpty);
    assert_eq!(read, bytes.len());
    // These explicitly supported codecs buffer at most three input bytes. Use
    // the codec itself to locate the complete prefix; do not duplicate its
    // UTF/DBCS validity rules or infer pending bytes from a U+FFFD in the text.
    // GB18030 can flush pending input as BOTH replacement and ASCII, so counting
    // only a decoder's malformed length is insufficient.
    let pending_bytes = (0..=bytes.len().min(3))
        .find(|pending| {
            let (candidate, _) = codec.decode_without_bom_handling(&bytes[..bytes.len() - pending]);
            candidate == text
        })
        .expect("supported stateless codec has a complete prefix within three bytes");
    DecodedOutput {
        text,
        consumed: bytes.len() - pending_bytes,
        pending_bytes,
        replacements,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_split_preserves_characters_and_byte_cursors() {
        let cases = [
            (
                OutputEncoding::Utf8,
                "中文😀�结束".as_bytes().to_vec(),
                "中文😀�结束",
            ),
            (
                OutputEncoding::Gbk,
                vec![0xd6, 0xd0, 0xce, 0xc4, 0xbd, 0xe1, 0xca, 0xf8],
                "中文结束",
            ),
            (OutputEncoding::Big5, vec![0xa4, 0xa4, 0xa4, 0xe5], "中文"),
            (OutputEncoding::Gb18030, vec![0x94, 0x39, 0xfc, 0x36], "😀"),
            (
                OutputEncoding::Utf16Le,
                vec![0x2d, 0x4e, 0x3d, 0xd8, 0x00, 0xde],
                "中😀",
            ),
            (
                OutputEncoding::Utf16Be,
                vec![0x4e, 0x2d, 0xd8, 0x3d, 0xde, 0x00],
                "中😀",
            ),
        ];
        for (encoding, bytes, expected) in cases {
            for split in 0..=bytes.len() {
                let first = decode_output(&bytes[..split], encoding, false);
                let rest = decode_output(&bytes[first.consumed..], encoding, true);
                assert_eq!(
                    format!("{}{}", first.text, rest.text),
                    expected,
                    "{encoding:?} split {split}"
                );
                assert!(!first.replacements && !rest.replacements);
                assert_eq!(first.consumed + rest.consumed, bytes.len());
            }
        }
    }

    #[test]
    fn malformed_and_incomplete_sequences_are_different() {
        let prefix = decode_output(&[0xff, 0xe4, 0xb8], OutputEncoding::Utf8, false);
        assert_eq!(prefix.text, "�");
        assert!(prefix.replacements);
        assert_eq!((prefix.consumed, prefix.pending_bytes), (1, 2));
        let eof = decode_output(&[0xe4, 0xb8], OutputEncoding::Utf8, true);
        assert_eq!(eof.text, "�");
        assert!(eof.replacements);
        assert_eq!(eof.consumed, 2);
        for bytes in [&[0x81, 0x30][..], &[0x81, 0x30, 0x81][..]] {
            let part = decode_output(bytes, OutputEncoding::Gb18030, false);
            assert_eq!(part.consumed, 0);
            assert_eq!(part.pending_bytes, bytes.len());
        }
        assert!(!decode_output("�".as_bytes(), OutputEncoding::Utf8, true).replacements);
    }

    #[test]
    fn utf16_offsets_are_explicit_and_empty_output_is_complete() {
        assert!(OutputEncoding::Utf16Le.validate_offset(1).is_err());
        assert_eq!(OutputEncoding::Utf16Be.align_start(3), 4);
        assert_eq!(decode_output(&[], OutputEncoding::Gbk, false).consumed, 0);
        let odd = decode_output(&[0x2d], OutputEncoding::Utf16Le, true);
        assert!(odd.replacements);
        assert_eq!(odd.consumed, 1);
    }

    #[test]
    fn malformed_prefixes_do_not_hide_or_duplicate_raw_bytes() {
        // Deterministic arbitrary bytes exercise malformed DBCS and surrogate
        // states, rather than constructing expected values with this decoder.
        for encoding in [
            OutputEncoding::Utf8,
            OutputEncoding::Gbk,
            OutputEncoding::Gb18030,
            OutputEncoding::Big5,
            OutputEncoding::Utf16Le,
            OutputEncoding::Utf16Be,
        ] {
            for seed in 0..256u16 {
                let bytes: Vec<_> = (0..31u16)
                    .map(|i| (seed.wrapping_mul(17).wrapping_add(i * 131) % 256) as u8)
                    .collect();
                let whole = decode_output(&bytes, encoding, true);
                for split in 0..=bytes.len() {
                    let first = decode_output(&bytes[..split], encoding, false);
                    let last = decode_output(&bytes[first.consumed..], encoding, true);
                    assert_eq!(
                        format!("{}{}", first.text, last.text),
                        whole.text,
                        "{encoding:?}/{seed}/{split}"
                    );
                }
            }
        }
    }
}
