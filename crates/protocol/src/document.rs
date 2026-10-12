use crate::{ExecutionSelection, RequestId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DOCUMENT_SCHEMA_VERSION: u16 = 1;
pub const MAX_DOCUMENT_TEXT_REPLY_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentOcrMode { Auto, Off, On }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentSelector {
    Overview,
    Page { number: u32 },
    Sheet { name: String },
    Table { number: u32 },
    Chunk { number: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentAction {
    Open { path: String, execution: ExecutionSelection, ocr_mode: DocumentOcrMode },
    Read { document_ref: RequestId, selector: DocumentSelector, cursor: Option<String> },
    Search { document_ref: RequestId, query: String, case_sensitive: bool, limit: u16, cursor: Option<String> },
    RenderPage { document_ref: RequestId, page_number: u32, max_dimension: u16 },
    Close { document_ref: RequestId },
}
impl DocumentAction {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Open { .. } => "document_open",
            Self::Read { .. } => "document_read",
            Self::Search { .. } => "document_search",
            Self::RenderPage { .. } => "document_render_page",
            Self::Close { .. } => "document_close",
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Open { path, execution, .. } => {
                if matches!(execution, ExecutionSelection::DesktopUser { .. }) {
                    return Err("document execution supports service or user");
                }
                if path.is_empty() || path.len() > 4096 || path.contains('\0') || path.contains("://") {
                    return Err("document path must be a local path of 1..4096 bytes");
                }
            }
            Self::Read { selector, cursor, .. } => {
                if cursor.as_ref().is_some_and(|v| v.len() > 256 || v.is_empty()) {
                    return Err("invalid document cursor");
                }
                match selector {
                    DocumentSelector::Page { number } | DocumentSelector::Table { number } | DocumentSelector::Chunk { number } if *number == 0 => return Err("document indices start at 1"),
                    DocumentSelector::Sheet { name } if name.is_empty() || name.len() > 255 => return Err("invalid sheet name"),
                    _ => {}
                }
            }
            Self::Search { query, limit, cursor, .. } => {
                if query.trim().is_empty() || query.len() > 512 || !(1..=50).contains(limit) || cursor.as_ref().is_some_and(|v| v.len() > 256 || v.is_empty()) {
                    return Err("search requires 1..512 bytes of text and limit 1..50");
                }
            }
            Self::RenderPage { page_number, max_dimension, .. } if *page_number == 0 || !(256..=2048).contains(max_dimension) => return Err("render page and dimension out of range"),
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentRequest {
    pub request_id: RequestId,
    pub operation: DocumentAction,
}
impl DocumentRequest {
    pub fn validate(&self) -> Result<(), &'static str> { self.operation.validate() }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentReply {
    pub request_id: RequestId,
    pub kind: String,
    pub state: String,
    pub document_ref: Option<RequestId>,
    pub data: Option<Value>,
    pub error: Option<String>,
}
impl DocumentReply {
    pub fn pending(request: &DocumentRequest) -> Self {
        Self { request_id: request.request_id, kind: request.operation.kind().into(), state: "running".into(), document_ref: None, data: None, error: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_urls_and_unbounded_queries() {
        assert!(DocumentAction::Open { path: "https://example.com/a.pdf".into(), execution: ExecutionSelection::Service {}, ocr_mode: DocumentOcrMode::Auto }.validate().is_err());
        assert!(DocumentAction::Search { document_ref: RequestId::new(), query: "x".repeat(513), case_sensitive: false, limit: 20, cursor: None }.validate().is_err());
    }
}
