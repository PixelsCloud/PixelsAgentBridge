//! Read-only, generation-bound system file clipboard. Native source paths never
//! cross the wire. Byte payloads follow these headers in bounded binary frames.
use crate::RequestId;
use serde::{Deserialize, Serialize};
pub const FILE_CLIPBOARD_VERSION: u16 = 1;
pub const FILE_CLIPBOARD_CHUNK: usize = 256 * 1024;
pub const FILE_CLIPBOARD_HEADER: usize = 8 * 1024 * 1024;
pub const FILE_CLIPBOARD_ITEMS: usize = 10000;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardFile {
    pub name: String,
    pub directory: bool,
    pub size: u64,
    pub modified: u64,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardFileOffer {
    pub id: RequestId,
    pub files: Vec<ClipboardFile>,
}
impl std::fmt::Debug for ClipboardFileOffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClipboardFileOffer(<redacted>)")
    }
}
impl ClipboardFileOffer {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.id.as_uuid().is_nil()
            || self.files.is_empty()
            || self.files.len() > FILE_CLIPBOARD_ITEMS
        {
            return Err("file_clipboard_invalid_offer");
        }
        let mut names = std::collections::HashMap::new();
        let mut bytes = 0usize;
        for file in &self.files {
            bytes = bytes
                .checked_add(file.name.len() + 64)
                .ok_or("file_clipboard_too_many_files")?;
            if bytes > FILE_CLIPBOARD_HEADER
                || file.name.encode_utf16().count() > 259
                || file.name.is_empty()
                || file.directory && file.size != 0
            {
                return Err("file_clipboard_invalid_name");
            }
            for component in file.name.split('\\') {
                let stem = component.split('.').next().unwrap_or("").to_uppercase();
                if component.is_empty()
                    || component == "."
                    || component == ".."
                    || component.ends_with(['.', ' '])
                    || component
                        .chars()
                        .any(|c| c < ' ' || "<>:\"/|?*".contains(c))
                    || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                    || ["COM", "LPT"].iter().any(|prefix| {
                        stem.strip_prefix(prefix).is_some_and(|s| {
                            matches!(
                                s,
                                "1" | "2"
                                    | "3"
                                    | "4"
                                    | "5"
                                    | "6"
                                    | "7"
                                    | "8"
                                    | "9"
                                    | "¹"
                                    | "²"
                                    | "³"
                            )
                        })
                    })
                {
                    return Err("file_clipboard_invalid_name");
                }
            }
            let key = file.name.to_lowercase();
            if let Some((parent, _)) = key.rsplit_once('\\') {
                if names.get(parent) != Some(&true) {
                    return Err("file_clipboard_invalid_tree");
                }
            }
            if names.insert(key, file.directory).is_some() {
                return Err("file_clipboard_duplicate_name");
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardFileRead {
    pub id: u64,
    pub stream: u64,
    pub batch: u64,
    pub batch_event: bool,
    pub offer: RequestId,
    pub index: u32,
    pub offset: u64,
    pub length: u32,
    pub end: bool,
}
impl ClipboardFileRead {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.offer.as_uuid().is_nil()
            || self.stream == 0
            || self.batch == 0
            || (!self.end
                && !self.batch_event
                && (self.id == 0
                    || self.length == 0
                    || self.length as usize > FILE_CLIPBOARD_CHUNK
                    || self.offset.checked_add(self.length as u64).is_none()))
        {
            return Err("file_clipboard_invalid_range");
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileClipboardOperation {
    Poll {
        after: Option<u64>,
    },
    Publish {
        offer: ClipboardFileOffer,
        expected_sequence: Option<u64>,
    },
    Read {
        request: ClipboardFileRead,
    },
    Deliver {
        id: u64,
        bytes: u32,
        error: Option<String>,
        #[serde(skip)]
        data: Vec<u8>,
    },
    Clear,
    Withdraw {
        offer: RequestId,
    },
}
impl FileClipboardOperation {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Publish { offer, .. } => offer.validate(),
            Self::Read { request } => request.validate(),
            Self::Deliver {
                id,
                bytes,
                error,
                data,
            } => {
                if *id == 0
                    || *bytes as usize > FILE_CLIPBOARD_CHUNK
                    || !data.is_empty() && data.len() != *bytes as usize
                    || error.as_ref().is_some_and(|e| e.len() > 128)
                {
                    Err("file_clipboard_invalid_reply")
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }
    pub fn payload(&self) -> &[u8] {
        match self {
            Self::Deliver { data, .. } => data,
            _ => &[],
        }
    }
}
impl std::fmt::Debug for FileClipboardOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileClipboardOperation(<redacted>)")
    }
}
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileClipboardSnapshot {
    pub sequence: u64,
    pub changed: bool,
    pub imported: bool,
    pub offer: Option<ClipboardFileOffer>,
    pub error: Option<String>,
    pub requests: Vec<ClipboardFileRead>,
    pub bytes: u32,
    pub sha256: Option<String>,
    #[serde(skip)]
    pub data: Vec<u8>,
}
impl FileClipboardSnapshot {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.requests.len() > 32
            || self.bytes as usize > FILE_CLIPBOARD_CHUNK
            || !self.data.is_empty() && self.data.len() != self.bytes as usize
            || self.error.as_ref().is_some_and(|e| e.len() > 128)
        {
            return Err("file_clipboard_invalid_reply");
        }
        if let Some(offer) = &self.offer {
            offer.validate()?;
        }
        for r in &self.requests {
            r.validate()?;
        }
        if self.bytes > 0
            && !self
                .sha256
                .as_ref()
                .is_some_and(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err("file_clipboard_invalid_digest");
        }
        Ok(())
    }
}
impl std::fmt::Debug for FileClipboardSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileClipboardSnapshot(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn offer(names: &[(&str, bool)]) -> ClipboardFileOffer {
        ClipboardFileOffer {
            id: RequestId::new(),
            files: names
                .iter()
                .map(|(name, directory)| ClipboardFile {
                    name: (*name).into(),
                    directory: *directory,
                    size: 0,
                    modified: 0,
                })
                .collect(),
        }
    }
    #[test]
    fn descriptors_reject_traversal_collisions_missing_parents_and_truncation() {
        assert!(
            offer(&[("中文😀", true), ("中文😀\\空文件", false)])
                .validate()
                .is_ok()
        );
        for names in [
            vec![("..\\bad", false)],
            vec![("C:\\secret", false)],
            vec![("a", false), ("A", false)],
            vec![("dir\\file", false)],
            vec![("dir", false), ("dir\\file", false)],
            vec![("CON.txt", false)],
            vec![("bad.", false)],
        ] {
            assert!(offer(&names).validate().is_err());
        }
        assert!(offer(&[(&"a".repeat(260), false)]).validate().is_err());
    }
    #[test]
    fn range_preserves_64_bits_and_rejects_overflow() {
        let mut r = ClipboardFileRead {
            id: 1,
            stream: 1,
            batch: 1,
            batch_event: false,
            offer: RequestId::new(),
            index: 0,
            offset: 5 * 1024 * 1024 * 1024,
            length: 32,
            end: false,
        };
        assert!(r.validate().is_ok());
        r.offset = u64::MAX;
        assert!(r.validate().is_err());
    }
}
