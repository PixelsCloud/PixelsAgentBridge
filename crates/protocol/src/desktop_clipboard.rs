//! Text only; clipboard contents are never debug output or configuration.
use serde::{Deserialize, Serialize};

pub const DESKTOP_CLIPBOARD_VERSION: u16 = 1;
pub const DESKTOP_CLIPBOARD_MAX_BYTES: usize = 1024 * 1024;
pub const DESKTOP_CLIPBOARD_CHUNK_BYTES: usize = 32 * 1024;
pub const DESKTOP_CLIPBOARD_JSON_BYTES: usize = DESKTOP_CLIPBOARD_MAX_BYTES * 6 + 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopClipboardRequest {
    Read { after: Option<u64> },
    Write { bytes: u32 },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopClipboardOperation {
    Read { after: Option<u64> },
    Write { text: String },
}
impl std::fmt::Debug for DesktopClipboardOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("DesktopClipboardOperation(<redacted>)") }
}
impl DesktopClipboardOperation {
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Self::Write { text } = self { validate_clipboard_text(text)?; }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopClipboardSnapshot {
    pub revision: u64,
    pub changed: bool,
    pub text: Option<String>,
}
impl std::fmt::Debug for DesktopClipboardSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("DesktopClipboardSnapshot(<redacted>)") }
}
impl DesktopClipboardSnapshot {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.revision == 0 || (!self.changed && self.text.is_some()) { return Err("invalid clipboard snapshot"); }
        if let Some(text) = &self.text { validate_clipboard_text(text)?; }
        Ok(())
    }
}
pub fn validate_clipboard_text(text: &str) -> Result<(), &'static str> {
    if text.len() > DESKTOP_CLIPBOARD_MAX_BYTES { return Err("clipboard text exceeds 1 MiB"); }
    if text.contains('\0') { return Err("clipboard text contains NUL"); }
    Ok(())
}
