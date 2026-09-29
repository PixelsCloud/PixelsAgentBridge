use serde::{Deserialize, Serialize};

use crate::RequestId;

pub const MAX_SCREENSHOT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SCREENSHOT_PIXELS: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotMeta {
    pub request_id: RequestId,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub size: u64,
    pub sha256: String,
}
