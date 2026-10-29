//! Clipboard headers are JSON; image bytes always travel in bounded binary frames.
use serde::{Deserialize, Serialize};

pub const DESKTOP_CLIPBOARD_VERSION: u16 = 2;
pub const DESKTOP_CLIPBOARD_MAX_BYTES: usize = 1024 * 1024;
pub const DESKTOP_CLIPBOARD_CHUNK_BYTES: usize = 32 * 1024;
pub const DESKTOP_CLIPBOARD_JSON_BYTES: usize = DESKTOP_CLIPBOARD_MAX_BYTES * 6 + 1024;
pub const DESKTOP_CLIPBOARD_IMAGE_BYTES: usize = 32 * 1024 * 1024;
pub const DESKTOP_CLIPBOARD_RGBA_BYTES: usize = 64 * 1024 * 1024;
pub const DESKTOP_CLIPBOARD_TIMEOUT_SECS: u64 = 60;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopClipboardImage {
    pub width: u32,
    pub height: u32,
    pub bytes: u32,
    #[serde(skip)]
    pub data: Vec<u8>,
}
impl std::fmt::Debug for DesktopClipboardImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("DesktopClipboardImage(<redacted>)") }
}
impl DesktopClipboardImage {
    pub fn validate_header(&self) -> Result<(), &'static str> {
        validate_clipboard_dimensions(self.width, self.height)?;
        if !(33..=DESKTOP_CLIPBOARD_IMAGE_BYTES).contains(&(self.bytes as usize)) { return Err("clipboard_image_too_large"); }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        self.validate_header()?;
        if self.data.len() != self.bytes as usize || self.data.get(..8) != Some(b"\x89PNG\r\n\x1a\n")
            || self.data.get(8..16) != Some(&[0,0,0,13,b'I',b'H',b'D',b'R'])
            || u32::from_be_bytes(self.data[16..20].try_into().unwrap()) != self.width
            || u32::from_be_bytes(self.data[20..24].try_into().unwrap()) != self.height {
            return Err("clipboard_image_invalid");
        }
        Ok(())
    }
}
pub fn validate_clipboard_dimensions(width: u32, height: u32) -> Result<(), &'static str> {
    if width == 0 || height == 0 || width > 16384 || height > 16384
        || u64::from(width) * u64::from(height) * 4 > DESKTOP_CLIPBOARD_RGBA_BYTES as u64 { return Err("clipboard_image_too_large"); }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopClipboardRequest {
    Read { after: Option<u64> },
    Write { bytes: u32 },
    WriteImage { image: DesktopClipboardImage, expected_revision: Option<u64> },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopClipboardOperation {
    Read { after: Option<u64> },
    Write { text: String },
    WriteImage { image: DesktopClipboardImage, expected_revision: Option<u64> },
}
impl std::fmt::Debug for DesktopClipboardOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("DesktopClipboardOperation(<redacted>)") }
}
impl DesktopClipboardOperation {
    pub fn image(&self) -> Option<&DesktopClipboardImage> { match self { Self::WriteImage { image, .. } => Some(image), _ => None } }
    pub fn image_mut(&mut self) -> Option<&mut DesktopClipboardImage> { match self { Self::WriteImage { image, .. } => Some(image), _ => None } }
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Self::Write { text } = self { validate_clipboard_text(text)?; }
        if let Self::WriteImage { image, .. } = self { image.validate()?; }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopClipboardSnapshot {
    pub revision: u64,
    pub changed: bool,
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<DesktopClipboardImage>,
}
impl std::fmt::Debug for DesktopClipboardSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("DesktopClipboardSnapshot(<redacted>)") }
}
impl DesktopClipboardSnapshot {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.revision == 0 || (!self.changed && (self.text.is_some() || self.image.is_some()))
            || (self.text.is_some() && self.image.is_some()) { return Err("invalid clipboard snapshot"); }
        if let Some(text) = &self.text { validate_clipboard_text(text)?; }
        if let Some(image) = &self.image { image.validate()?; }
        Ok(())
    }
}
pub fn validate_clipboard_text(text: &str) -> Result<(), &'static str> {
    if text.len() > DESKTOP_CLIPBOARD_MAX_BYTES { return Err("clipboard text exceeds 1 MiB"); }
    if text.contains('\0') { return Err("clipboard text contains NUL"); }
    Ok(())
}
