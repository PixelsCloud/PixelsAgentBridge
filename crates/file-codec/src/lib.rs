//! Shared bounded text decoding, ranges and conflict-safe patch planning.
//! Storage, authorization and publication belong to the platform adapter.

pub mod archive;
#[cfg(test)]
mod enhanced_tests;
pub mod log;
pub mod search;
mod text;
pub use text::{Document, decode, encode, newline, patch, read, validate_text};

pub struct PatchPlan {
    pub bytes: Vec<u8>,
    pub encoding: pab_protocol::TextEncoding,
    pub newline: String,
    pub preview: pab_protocol::PatchPreview,
}

pub fn plan_patch(
    original: &[u8],
    encoding: Option<pab_protocol::TextEncoding>,
    edits: &[pab_protocol::TextEdit],
) -> Result<PatchPlan, FileError> {
    use sha2::{Digest, Sha256};
    let doc = decode(original, encoding)?;
    let updated = patch(&doc.text, edits)?;
    let bytes = encode(&updated, doc.encoding, doc.bom > 0)?;
    let preview = pab_protocol::PatchPreview {
        original_sha256: format!("{:x}", Sha256::digest(original)),
        result_sha256: format!("{:x}", Sha256::digest(&bytes)),
        result_size: bytes.len() as u64,
        changed: original != bytes,
        matched_edits: edits.iter().map(|e| e.expected_matches).collect(),
    };
    Ok(PatchPlan {
        bytes,
        encoding: doc.encoding,
        newline: newline(&updated),
        preview,
    })
}

#[derive(Debug)]
pub struct FileError(pub pab_protocol::FileSystemError);

impl std::fmt::Display for FileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.0.code, self.0.message)
    }
}
impl std::error::Error for FileError {}

impl FileError {
    pub fn new(code: &str, phase: &str, message: impl Into<String>) -> Self {
        Self(pab_protocol::FileSystemError {
            code: code.to_owned(),
            phase: phase.to_owned(),
            message: message.into().chars().take(2048).collect(),
        })
    }
}
