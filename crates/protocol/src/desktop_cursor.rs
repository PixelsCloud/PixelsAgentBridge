//! Independent cursor metadata. Pixels use a bounded binary payload, never JSON.
use crate::{DesktopVideoConfig, RequestId};
use serde::{Deserialize, Serialize};

pub const DESKTOP_CURSOR_SCHEMA_VERSION: u16 = 1;
pub const DESKTOP_MAX_CURSOR_EDGE: u32 = 512;
pub const DESKTOP_MAX_CURSOR_BYTES: usize = 512 * 512 * 8;

/// Capture adapters must explicitly select one path to avoid drawing twice.
/// Availability is negotiated before opening a cursor stream; this enum alone
/// does not advertise a new capability on existing desktop sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopCursorMode {
    Embedded,
    Separate,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopCursorFormat {
    /// Top-down rows of BGRA with premultiplied alpha, no row padding.
    PremultipliedBgra,
    /// Top-down pixels, each with four AND bytes then four XOR bytes. The
    /// compositor applies (background.rgb & and.rgb) ^ xor.rgb; alpha bytes
    /// are ignored. Preserves classic Windows inverted/monochrome cursors,
    /// which cannot be faithfully represented by ordinary alpha blending.
    AndXorBgra,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopCursorImage {
    pub format: DesktopCursorFormat,
    /// Image pixels are in the already-oriented visible capture pixel space.
    /// Scale with the viewport; do not rotate the cursor a second time.
    pub width: u32,
    pub height: u32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
}
impl DesktopCursorImage {
    pub fn payload_bytes(&self) -> Result<usize, &'static str> {
        if self.width == 0
            || self.height == 0
            || self.width > DESKTOP_MAX_CURSOR_EDGE
            || self.height > DESKTOP_MAX_CURSOR_EDGE
            || self.hotspot_x >= self.width
            || self.hotspot_y >= self.height
        {
            return Err("invalid desktop cursor dimensions or hotspot");
        }
        let stride = match self.format {
            DesktopCursorFormat::PremultipliedBgra => 4,
            DesktopCursorFormat::AndXorBgra => 8,
        };
        // Dimension checks precede multiplication, including on 32-bit hosts.
        Ok(self.width as usize * self.height as usize * stride)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopCursorPosition {
    /// Hotspot in the same desktop coordinate space as DesktopScreen.x/y and
    /// input_position. Negative origins are valid. Clip in the renderer using
    /// wide arithmetic; positions outside the selected screen never select a
    /// different screen or alter input routing.
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopCursorHeader {
    pub version: u16,
    pub session_id: RequestId,
    pub generation: u64,
    pub sequence: u64,
    /// None means hidden, including cursor embedded in the current video.
    pub position: Option<DesktopCursorPosition>,
    /// Generation-local monotonic ID, not an OS cursor handle. Zero is only
    /// valid while hidden before any image has been supplied.
    pub image_id: u64,
    /// A new image replaces the single cached image. Position-only updates
    /// reference that image without retransmitting its bytes. No growing cache.
    pub image: Option<DesktopCursorImage>,
}
impl DesktopCursorHeader {
    /// Returns the exact binary length after validating identity and state,
    /// before the receiver allocates or waits for an image payload.
    pub fn validate(
        &self,
        config: &DesktopVideoConfig,
        next_sequence: u64,
        current_image_id: u64,
    ) -> Result<usize, &'static str> {
        if self.version != DESKTOP_CURSOR_SCHEMA_VERSION
            || self.session_id != config.session_id
            || self.generation != config.generation
            || next_sequence == 0
            || self.sequence != next_sequence
        {
            return Err("invalid desktop cursor identity or sequence");
        }
        if self.position.is_some() && self.image_id == 0 {
            return Err("visible desktop cursor requires an image");
        }
        match &self.image {
            Some(image) => {
                if self.image_id <= current_image_id {
                    return Err("desktop cursor image ID must advance");
                }
                image.payload_bytes()
            }
            None => {
                if self.image_id != current_image_id {
                    return Err("desktop cursor references an unknown image");
                }
                Ok(0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_image_bounds_cover_color_masks_and_hotspot_edges() {
        let mut image = DesktopCursorImage {
            format: DesktopCursorFormat::AndXorBgra,
            width: 512,
            height: 512,
            hotspot_x: 511,
            hotspot_y: 511,
        };
        assert_eq!(image.payload_bytes().unwrap(), DESKTOP_MAX_CURSOR_BYTES);
        image.format = DesktopCursorFormat::PremultipliedBgra;
        assert_eq!(image.payload_bytes().unwrap(), 1024 * 1024);
        for edge in [0, 513, u32::MAX] {
            let mut bad = image.clone();
            bad.width = edge;
            assert!(bad.payload_bytes().is_err());
            bad = image.clone();
            bad.height = edge;
            assert!(bad.payload_bytes().is_err());
        }
        image.hotspot_x = image.width;
        assert!(image.payload_bytes().is_err());
        image.hotspot_x = 0;
        image.hotspot_y = image.height;
        assert!(image.payload_bytes().is_err());
    }
}
