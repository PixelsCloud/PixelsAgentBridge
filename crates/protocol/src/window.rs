use serde::{Deserialize, Serialize};

use crate::RequestId;

pub const MAX_WINDOW_ENTRIES: usize = 64;
pub const MAX_WINDOW_TITLE_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowEntry {
    pub title: String,
    pub process_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowList {
    pub request_id: RequestId,
    pub entries: Vec<WindowEntry>,
}
