use serde::{Deserialize, Serialize};

use crate::RequestId;

pub const MAX_SCREENSHOT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SCREENSHOT_PIXELS: u64 = 16 * 1024 * 1024;
pub const SCREENSHOT_SCHEMA_VERSION: u16 = 3;
pub const MAX_INLINE_SCREENSHOT_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotMode {
    Preview,
    #[default]
    Jpeg,
    Original,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotFormat {
    Png,
    Jpeg,
}
impl ScreenshotFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
        }
    }
    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScreenshotOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_ref: Option<String>,
    pub mode: ScreenshotMode,
    pub format: Option<ScreenshotFormat>,
    pub monitor_id: Option<u32>,
    pub region: Option<ScreenshotRegion>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub max_bytes: Option<u32>,
    pub quality: Option<u8>,
}
impl ScreenshotOptions {
    pub fn required_version(&self) -> u16 {
        if self.mode == ScreenshotMode::Jpeg {
            3
        } else if self.window_ref.is_some() {
            2
        } else {
            1
        }
    }
    pub fn legacy_preview() -> Self {
        Self {
            mode: ScreenshotMode::Preview,
            ..Self::default()
        }
    }
    pub fn original() -> Self {
        Self {
            mode: ScreenshotMode::Original,
            ..Self::default()
        }
    }
    pub fn format(&self) -> ScreenshotFormat {
        self.format.unwrap_or(match self.mode {
            ScreenshotMode::Preview | ScreenshotMode::Jpeg => ScreenshotFormat::Jpeg,
            ScreenshotMode::Original => ScreenshotFormat::Png,
        })
    }
    pub fn byte_limit(&self) -> usize {
        if self.mode == ScreenshotMode::Jpeg {
            return usize::MAX;
        }
        self.max_bytes
            .map(|v| v as usize)
            .unwrap_or(match self.mode {
                ScreenshotMode::Preview => MAX_INLINE_SCREENSHOT_BYTES,
                ScreenshotMode::Original => MAX_SCREENSHOT_BYTES,
                ScreenshotMode::Jpeg => unreachable!(),
            })
    }
    pub fn bounds(&self) -> (u32, u32) {
        (
            self.max_width.unwrap_or(1600),
            self.max_height.unwrap_or(1000),
        )
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.mode == ScreenshotMode::Jpeg
            && (self.format() != ScreenshotFormat::Jpeg
                || self.max_width.is_some()
                || self.max_height.is_some()
                || self.max_bytes.is_some())
        {
            return Err(
                "native JPEG preserves resolution and does not accept resize/byte limits or other formats",
            );
        }

        if let Some(reference) = &self.window_ref {
            if reference.parse::<RequestId>().is_err() {
                return Err("window_ref must be an opaque window reference");
            }
            if self.monitor_id.is_some()
                || self.region.is_some()
                || self.mode == ScreenshotMode::Original
            {
                return Err("monitor_id/region/original cannot be combined with window_ref");
            }
        }
        if (self.mode != ScreenshotMode::Jpeg
            && !(16 * 1024..=MAX_SCREENSHOT_BYTES).contains(&self.byte_limit()))
            || self.max_width.is_some_and(|v| !(64..=8192).contains(&v))
            || self.max_height.is_some_and(|v| !(64..=8192).contains(&v))
            || self.quality.is_some_and(|v| !(30..=95).contains(&v))
        {
            return Err("max_bytes 16384..8388608, preview bounds 64..8192, JPEG quality 30..95");
        }
        if self.mode == ScreenshotMode::Original
            && (self.max_width.is_some() || self.max_height.is_some())
        {
            return Err("original mode does not accept resize bounds");
        }
        if self.format() == ScreenshotFormat::Png && self.quality.is_some() {
            return Err("quality applies only to JPEG");
        }
        if let Some(r) = &self.region
            && (r.width == 0
                || r.height == 0
                || r.width
                    > if self.mode == ScreenshotMode::Jpeg {
                        65535
                    } else {
                        16384
                    }
                || r.height
                    > if self.mode == ScreenshotMode::Jpeg {
                        65535
                    } else {
                        16384
                    }
                || (self.mode != ScreenshotMode::Jpeg
                    && u64::from(r.width) * u64::from(r.height) > MAX_SCREENSHOT_PIXELS)
                || r.x.checked_add(r.width).is_none()
                || r.y.checked_add(r.height).is_none())
        {
            return Err("region must have positive bounded dimensions without coordinate overflow");
        }
        Ok(())
    }
}
/// Window client rectangle in xcap native desktop coordinates, independent of image pixel size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotDesktopRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_rect: Option<ScreenshotDesktopRect>,
    pub captured_at_unix_ms: i64,
    pub mode: ScreenshotMode,
    pub format: ScreenshotFormat,
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub monitor_id: Option<u32>,
    pub origin_x: i32,
    pub origin_y: i32,
    pub quality: Option<u8>,
    pub resized: bool,
}
impl ScreenshotInfo {
    pub fn validate(&self, options: &ScreenshotOptions) -> Result<(), &'static str> {
        options.validate()?;
        if self.window_ref != options.window_ref {
            return Err("screenshot window identity mismatch");
        }
        match (&self.window_ref, &self.desktop_rect) {
            (Some(_), Some(rect))
                if rect.width > 0
                    && rect.height > 0
                    && rect.width <= 65535
                    && rect.height <= 65535
                    && (rect.x, rect.y) == (self.origin_x, self.origin_y)
                    && i64::from(rect.x) + i64::from(rect.width) <= i64::from(i32::MAX)
                    && i64::from(rect.y) + i64::from(rect.height) <= i64::from(i32::MAX) => {}
            (None, None) => {}
            _ => return Err("invalid window desktop rectangle"),
        }
        if self.mode != options.mode
            || self.format != options.format()
            || self.width == 0
            || self.height == 0
            || self.source_width == 0
            || self.source_height == 0
            || self.source_width
                > if self.mode == ScreenshotMode::Jpeg {
                    65535
                } else {
                    16384
                }
            || self.source_height
                > if self.mode == ScreenshotMode::Jpeg {
                    65535
                } else {
                    16384
                }
            || (self.mode != ScreenshotMode::Jpeg
                && u64::from(self.source_width) * u64::from(self.source_height)
                    > MAX_SCREENSHOT_PIXELS)
            || self.width > self.source_width
            || self.height > self.source_height
            || self.resized
                != (self.width != self.source_width || self.height != self.source_height)
        {
            return Err("screenshot metadata contradicts capture options or dimensions");
        }
        if options
            .monitor_id
            .is_some_and(|id| self.monitor_id != Some(id))
        {
            return Err("screenshot monitor identity mismatch");
        }
        if let Some(r) = &options.region
            && (self.source_width, self.source_height) != (r.width, r.height)
        {
            return Err("screenshot region dimensions mismatch");
        }
        if matches!(self.mode, ScreenshotMode::Original | ScreenshotMode::Jpeg) && self.resized {
            return Err("original screenshot was resized");
        }
        if self.mode == ScreenshotMode::Preview {
            let (w, h) = options.bounds();
            if self.width > w || self.height > h {
                return Err("preview exceeds dimension budget");
            }
        }
        if self.format == ScreenshotFormat::Png && self.quality.is_some()
            || self.format == ScreenshotFormat::Jpeg
                && self.quality.is_none_or(|v| !(30..=95).contains(&v))
        {
            return Err("invalid encoded JPEG quality metadata");
        }
        if self.format == ScreenshotFormat::Jpeg {
            let requested = options.quality.unwrap_or(
                if matches!(self.mode, ScreenshotMode::Original | ScreenshotMode::Jpeg) {
                    85
                } else {
                    75
                },
            );
            if self.quality.is_some_and(|quality| {
                quality > requested
                    || (matches!(self.mode, ScreenshotMode::Original | ScreenshotMode::Jpeg)
                        && quality != requested)
            }) {
                return Err("encoded quality contradicts requested JPEG quality");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotMeta {
    pub request_id: RequestId,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub size: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<ScreenshotInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_jpeg_selection_requires_v3_and_does_not_accept_other_sources() {
        let r = RequestId::new().to_string();
        let good = ScreenshotOptions {
            window_ref: Some(r),
            ..Default::default()
        };
        assert!(good.validate().is_ok());
        assert_eq!(good.required_version(), 3);
        assert_eq!(ScreenshotOptions::default().required_version(), 3);
        assert_eq!(ScreenshotOptions::legacy_preview().required_version(), 1);
        assert_eq!(
            ScreenshotOptions {
                window_ref: good.window_ref.clone(),
                ..ScreenshotOptions::legacy_preview()
            }
            .required_version(),
            2
        );
        let mut q = good.clone();
        q.monitor_id = Some(1);
        assert!(q.validate().is_err());
        q = good.clone();
        q.region = Some(ScreenshotRegion {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        });
        assert!(q.validate().is_err());
        q = good.clone();
        q.mode = ScreenshotMode::Original;
        assert!(q.validate().is_err());
        q = good.clone();
        q.max_bytes = Some(524289);
        assert!(q.validate().is_err());
        q = good;
        q.window_ref = Some("raw-hwnd".into());
        assert!(q.validate().is_err());
    }
    #[test]
    fn options_default_and_old_metadata_remain_compatible() {
        let options: ScreenshotOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(options.format(), ScreenshotFormat::Jpeg);
        assert!(
            serde_json::to_value(&options)
                .unwrap()
                .get("window_ref")
                .is_none()
        );
        assert_eq!(options.mode, ScreenshotMode::Jpeg);
        assert_eq!(options.byte_limit(), usize::MAX);
        assert_eq!(
            ScreenshotOptions::legacy_preview().byte_limit(),
            MAX_INLINE_SCREENSHOT_BYTES
        );
        let meta: ScreenshotMeta = serde_json::from_value(serde_json::json!({
            "request_id": RequestId::new(), "format":"png", "width":1,"height":1,"size":1,"sha256":"test"
        })).unwrap();
        assert_eq!(meta.capture, None);
        assert!(serde_json::to_value(meta).unwrap().get("capture").is_none());
    }
    #[test]
    fn options_reject_overflow_unsupported_fields_and_conflicting_modes() {
        for json in [
            r#"{"region":{"x":4294967295,"y":0,"width":2,"height":1}}"#,
            r#"{"mode":"original","max_width":100}"#,
            r#"{"format":"png","quality":75}"#,
            r#"{"max_bytes":10}"#,
            r#"{"quality":96}"#,
        ] {
            assert!(
                serde_json::from_str::<ScreenshotOptions>(json)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        assert!(serde_json::from_str::<ScreenshotOptions>(r#"{"window_id":1}"#).is_err());
    }
}
