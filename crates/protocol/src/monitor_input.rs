use crate::{DesktopMouseButton, RequestId};
use serde::{Deserialize, Serialize};

/// xcap uses physical desktop pixels on Windows/X11 and points on macOS.
/// Input coordinates are always logical units relative to the selected monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorCoordinateSpace {
    PhysicalPixels,
    LogicalPoints,
}

/// Copy this snapshot from list_monitors. It is not a durable monitor identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorTarget {
    pub helper_instance: String,
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_percent: u32,
    pub rotation_degrees: u16,
    pub coordinate_space: MonitorCoordinateSpace,
}

impl MonitorTarget {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.helper_instance.parse::<RequestId>().is_err() {
            return Err("invalid helper_instance; list monitors again");
        }
        if self.width == 0
            || self.height == 0
            || self.width > 65535
            || self.height > 65535
            || !(1..=800).contains(&self.scale_percent)
            || self.rotation_degrees > 360
            || i64::from(self.x) + i64::from(self.width) > i64::from(i32::MAX)
            || i64::from(self.y) + i64::from(self.height) > i64::from(i32::MAX)
        {
            return Err("invalid monitor geometry or scaling");
        }
        Ok(())
    }
    /// Round to the nearest native unit, then reject outside coordinates, never clip.
    pub fn native_point(&self, x: u32, y: u32) -> Result<(i32, i32), &'static str> {
        self.validate()?;
        let factor = match self.coordinate_space {
            MonitorCoordinateSpace::PhysicalPixels => self.scale_percent,
            MonitorCoordinateSpace::LogicalPoints => 100,
        };
        let nx = (u64::from(x) * u64::from(factor) + 50) / 100;
        let ny = (u64::from(y) * u64::from(factor) + 50) / 100;
        if nx >= u64::from(self.width) || ny >= u64::from(self.height) {
            return Err("input point is outside the selected monitor");
        }
        Ok((
            self.x.checked_add(nx as i32).ok_or("input x overflow")?,
            self.y.checked_add(ny as i32).ok_or("input y overflow")?,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MonitorAction {
    Move {
        x: u32,
        y: u32,
    },
    Click {
        x: u32,
        y: u32,
        button: DesktopMouseButton,
    },
}
impl MonitorAction {
    pub fn point(&self) -> (u32, u32) {
        match *self {
            Self::Move { x, y } | Self::Click { x, y, .. } => (x, y),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorInput {
    pub target: MonitorTarget,
    pub action: MonitorAction,
}
impl MonitorInput {
    pub fn validate(&self) -> Result<(), &'static str> {
        let (x, y) = self.action.point();
        self.target.native_point(x, y).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target(x: i32, y: i32, width: u32, height: u32, scale: u32, points: bool) -> MonitorTarget {
        MonitorTarget {
            helper_instance: RequestId::new().to_string(),
            id: 42,
            x,
            y,
            width,
            height,
            scale_percent: scale,
            rotation_degrees: 0,
            coordinate_space: if points {
                MonitorCoordinateSpace::LogicalPoints
            } else {
                MonitorCoordinateSpace::PhysicalPixels
            },
        }
    }
    #[test]
    fn simulated_layouts_have_independent_expected_coordinates() {
        for (t, xy, expected) in [
            (target(-1920, 0, 1920, 1080, 100, false), (0, 0), (-1920, 0)),
            (
                target(-1920, 0, 1920, 1080, 100, false),
                (1919, 1079),
                (-1, 1079),
            ),
            (
                target(1920, -500, 2560, 1440, 150, false),
                (100, 200),
                (2070, -200),
            ),
            (
                target(0, -2160, 3840, 2160, 200, false),
                (1919, 1079),
                (3838, -2),
            ),
            (
                target(-1440, -900, 1440, 900, 200, true),
                (100, 200),
                (-1340, -700),
            ),
            (
                target(0, 1080, 1080, 1920, 100, false),
                (1079, 1919),
                (1079, 2999),
            ),
        ] {
            assert_eq!(t.native_point(xy.0, xy.1), Ok(expected));
        }
    }
    #[test]
    fn rejects_edges_overflow_empty_and_invalid_scale_before_input() {
        let t = target(0, 0, 1920, 1080, 150, false);
        for xy in [(1280, 0), (0, 720), (u32::MAX, u32::MAX)] {
            assert!(t.native_point(xy.0, xy.1).is_err());
        }
        for t in [
            target(i32::MAX, 0, 2, 2, 100, false),
            target(0, 0, 0, 1, 100, false),
            target(0, 0, 2, 2, 0, false),
            target(0, 0, 2, 2, 801, false),
        ] {
            assert!(t.validate().is_err());
        }
        for v in [
            serde_json::json!({"type":"move","x":-1,"y":0}),
            serde_json::json!({"type":"move","x":1.5,"y":0}),
            serde_json::json!({"type":"move","x":null,"y":0}),
        ] {
            assert!(serde_json::from_value::<MonitorAction>(v).is_err());
        }
    }
}
