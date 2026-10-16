//! Additive reliable-stream remote desktop contract. No credentials or pixels
//! belong in these JSON messages; compressed video uses a separate binary flow.
use crate::RequestId;
use serde::{Deserialize, Serialize};

pub const REMOTE_DESKTOP_SCHEMA_VERSION: u16 = 1;
pub const DESKTOP_VIDEO_CHUNK_BYTES: usize = 64 * 1024;
pub const DESKTOP_MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub const DESKTOP_MAX_IN_FLIGHT: u64 = 4;
pub const DESKTOP_CONTROL_BYTES: usize = 64 * 1024;
pub const DESKTOP_MAX_SCREENS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopRotation {
    Identity,
    Clockwise90,
    Clockwise180,
    Clockwise270,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopScreen {
    /// Backend-stable ID within the current desktop, never an enumeration index.
    pub id: String,
    pub name: String,
    /// Native desktop coordinates; may be negative. Input mapping also uses
    /// desktop size, source pixel size and rotation, not an assumed 1:1 scale.
    pub x: i32,
    pub y: i32,
    pub desktop_width: u32,
    pub desktop_height: u32,
    /// Unrotated capture/encoded texture size. Rotation maps it onto the desktop.
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub scale_milli: u32,
    pub rotation: DesktopRotation,
    pub primary: bool,
}
fn dimensions(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= 8192
        && height <= 8192
        && u64::from(width) * u64::from(height) <= 33_554_432
}
pub fn validate_desktop_screen_id(id: &str) -> Result<(), &'static str> {
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err("invalid desktop screen ID");
    }
    Ok(())
}
impl DesktopScreen {
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_desktop_screen_id(&self.id)?;
        if self.name.len() > 512
            || self.name.chars().any(char::is_control)
            || !dimensions(self.pixel_width, self.pixel_height)
            || !dimensions(self.desktop_width, self.desktop_height)
            || !(100..=8000).contains(&self.scale_milli)
            || self.x.checked_add(self.desktop_width as i32).is_none()
            || self.y.checked_add(self.desktop_height as i32).is_none()
        {
            return Err("invalid desktop screen layout");
        }
        Ok(())
    }
}
pub fn validate_desktop_screens(screens: &[DesktopScreen]) -> Result<(), &'static str> {
    if screens.is_empty() || screens.len() > DESKTOP_MAX_SCREENS {
        return Err("invalid screen list size");
    }
    let mut ids = std::collections::HashSet::new();
    for screen in screens {
        screen.validate()?;
        if !ids.insert(&screen.id) {
            return Err("duplicate screen ID");
        }
    }
    if screens.iter().filter(|s| s.primary).count() > 1 {
        return Err("multiple primary screens");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopCodec {
    H264AnnexB,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopColor {
    Bt709Limited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopVideoConfig {
    pub session_id: RequestId,
    /// Increment on screen/layout/codec changes or video restart. A new stream
    /// always starts at frame 1 with a decoder-initializing keyframe (SPS/PPS).
    pub generation: u64,
    pub screen: DesktopScreen,
    pub codec: DesktopCodec,
    pub color: DesktopColor,
    pub fps: u16,
}
impl DesktopVideoConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.session_id.as_uuid().is_nil()
            || self.generation == 0
            || !(1..=60).contains(&self.fps)
        {
            return Err("invalid desktop video configuration");
        }
        self.screen.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopOpened {
    pub config: DesktopVideoConfig,
    pub screens: Vec<DesktopScreen>,
    pub heartbeat_ms: u32,
    pub lease_ms: u32,
}
impl DesktopOpened {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.config.validate()?;
        validate_desktop_screens(&self.screens)?;
        if !self.screens.contains(&self.config.screen)
            || self.heartbeat_ms == 0
            || self.heartbeat_ms > 5000
            || self.lease_ms < self.heartbeat_ms * 2
            || self.lease_ms > 30000
        {
            return Err("invalid desktop open reply");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteDesktopRequest {
    Open {
        version: u16,
        session_id: RequestId,
        screen_id: Option<String>,
        fps: u16,
    },
    Control {
        session_id: RequestId,
    },
    Video {
        session_id: RequestId,
        generation: u64,
    },
    Close {
        session_id: RequestId,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopErrorCode {
    Unsupported,
    InvalidRequest,
    Busy,
    NotFound,
    WrongOwner,
    StaleGeneration,
    MediaUnavailable,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteDesktopReply {
    Opened {
        opened: DesktopOpened,
    },
    Ready,
    Closed,
    Error {
        code: DesktopErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopControl {
    pub sequence: u64,
    pub command: DesktopControlCommand,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopControlCommand {
    Heartbeat,
    /// Also restarts a failed/lagging stream on the same screen. Never replays input.
    SelectScreen {
        generation: u64,
        screen_id: String,
    },
    /// Cumulative decoded/presented consumption, not merely network receipt.
    Presented {
        generation: u64,
        frame_id: u64,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopControlReply {
    Applied {
        sequence: u64,
    },
    Configured {
        sequence: u64,
        opened: DesktopOpened,
    },
    Error {
        sequence: u64,
        code: DesktopErrorCode,
        message: String,
    },
}
impl DesktopControlReply {
    pub fn sequence(&self) -> u64 {
        match self {
            Self::Applied { sequence }
            | Self::Configured { sequence, .. }
            | Self::Error { sequence, .. } => *sequence,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopFrameHeader {
    pub version: u16,
    pub session_id: RequestId,
    pub generation: u64,
    pub frame_id: u64,
    /// Producer monotonic timestamp in microseconds; clocks are not assumed synced.
    pub capture_time_us: u64,
    pub keyframe: bool,
    pub payload_bytes: u32,
}
impl DesktopFrameHeader {
    pub fn validate(
        &self,
        config: &DesktopVideoConfig,
        next_frame: u64,
    ) -> Result<(), &'static str> {
        if self.version != REMOTE_DESKTOP_SCHEMA_VERSION
            || self.session_id != config.session_id
            || self.generation != config.generation
            || self.frame_id != next_frame
            || next_frame == 0
            || (next_frame == 1 && !self.keyframe)
            || self.payload_bytes == 0
            || self.payload_bytes as usize > DESKTOP_MAX_FRAME_BYTES
        {
            return Err("invalid desktop frame identity, sequence, keyframe or length");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn screen_layout_keeps_negative_origins_rotation_and_scaling() {
        let s = DesktopScreen {
            id: "physical-output-2".into(),
            name: "Portrait".into(),
            x: -720,
            y: -50,
            desktop_width: 720,
            desktop_height: 1280,
            pixel_width: 1920,
            pixel_height: 1080,
            scale_milli: 1500,
            rotation: DesktopRotation::Clockwise90,
            primary: false,
        };
        validate_desktop_screens(&[s.clone()]).unwrap();
        assert!(validate_desktop_screens(&[s.clone(), s.clone()]).is_err());
        let mut bad = s;
        bad.x = i32::MAX;
        assert!(bad.validate().is_err());
    }
}
