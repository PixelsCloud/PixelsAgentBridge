#![forbid(unsafe_code)]
pub use image;
use image::{
    DynamicImage, ImageEncoder, ImageFormat, ImageReader, Limits,
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
    imageops::FilterType,
};
use pab_protocol::*;
use std::io::{self, Cursor, Write};

#[derive(Debug)]
pub struct EncodedScreenshot {
    pub info: ScreenshotInfo,
    pub bytes: Vec<u8>,
}
struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
    overflow: bool,
}
impl Write for BoundedWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.overflow = true;
            return Err(io::Error::other("encoded screenshot exceeds byte budget"));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn format(f: ScreenshotFormat) -> ImageFormat {
    match f {
        ScreenshotFormat::Png => ImageFormat::Png,
        ScreenshotFormat::Jpeg => ImageFormat::Jpeg,
    }
}
/// Input is already captured/cropped; this function never changes desktop selection.
pub fn encode(
    image: DynamicImage,
    options: &ScreenshotOptions,
    monitor_id: Option<u32>,
    origin: (i32, i32),
) -> Result<EncodedScreenshot, String> {
    options.validate().map_err(str::to_owned)?;
    let (source_width, source_height) = (image.width(), image.height());
    if source_width == 0
        || source_height == 0
        || u64::from(source_width) * u64::from(source_height) > MAX_SCREENSHOT_PIXELS
    {
        return Err("capture dimensions exceed pixel budget".into());
    }
    let mut current = if options.mode == ScreenshotMode::Preview {
        let (w, h) = options.bounds();
        if source_width > w || source_height > h {
            image.resize(w, h, FilterType::Triangle)
        } else {
            image
        }
    } else {
        image
    };
    let mut quality = options
        .quality
        .unwrap_or(if options.mode == ScreenshotMode::Original {
            85
        } else {
            75
        });
    for _ in 0..10 {
        let mut writer = BoundedWriter {
            bytes: Vec::new(),
            limit: options.byte_limit(),
            overflow: false,
        };
        let result = match options.format() {
            ScreenshotFormat::Png => PngEncoder::new(&mut writer).write_image(
                current.as_bytes(),
                current.width(),
                current.height(),
                current.color().into(),
            ),
            ScreenshotFormat::Jpeg => {
                JpegEncoder::new_with_quality(&mut writer, quality).encode_image(&current.to_rgb8())
            }
        };
        match result {
            Ok(()) => {
                let info = ScreenshotInfo {
                    captured_at_unix_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                        .min(i64::MAX as u128) as i64,
                    mode: options.mode,
                    format: options.format(),
                    width: current.width(),
                    height: current.height(),
                    source_width,
                    source_height,
                    monitor_id,
                    origin_x: origin.0,
                    origin_y: origin.1,
                    quality: (options.format() == ScreenshotFormat::Jpeg).then_some(quality),
                    resized: source_width != current.width() || source_height != current.height(),
                };
                info.validate(options).map_err(str::to_owned)?;
                return Ok(EncodedScreenshot {
                    info,
                    bytes: writer.bytes,
                });
            }
            Err(e) if !writer.overflow => return Err(e.to_string()),
            Err(_) if options.mode == ScreenshotMode::Original => {
                return Err(
                    "original encoding exceeds max_bytes; request preview or a larger byte budget"
                        .into(),
                );
            }
            Err(_) => {}
        }
        if options.format() == ScreenshotFormat::Jpeg && quality > 45 {
            quality = if quality > 60 { 60 } else { 45 };
            continue;
        }
        if current.width() <= 64 && current.height() <= 64 {
            break;
        }
        let w = (current.width() * 3 / 4).max(1);
        let h = (current.height() * 3 / 4).max(1);
        current = current.resize(w, h, FilterType::Triangle);
    }
    Err("preview could not fit max_bytes within 10 bounded encoding attempts".into())
}
/// Strict dimension limits plus best-effort decoder allocation limits; not a hard process memory quota.
fn reader(bytes: &[u8], f: ScreenshotFormat) -> ImageReader<Cursor<&[u8]>> {
    let mut r = ImageReader::with_format(Cursor::new(bytes), format(f));
    let mut limits = Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    r.limits(limits);
    r
}
pub fn dimensions(bytes: &[u8], f: ScreenshotFormat) -> Result<(u32, u32), String> {
    if bytes.is_empty()
        || bytes.len() > MAX_SCREENSHOT_BYTES
        || image::guess_format(bytes).map_err(|e| e.to_string())? != format(f)
    {
        return Err("invalid screenshot format or encoded size".into());
    }
    let (w, h) = reader(bytes, f)
        .into_dimensions()
        .map_err(|e| e.to_string())?;
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_SCREENSHOT_PIXELS {
        return Err("screenshot exceeds pixel budget".into());
    }
    Ok((w, h))
}
pub fn verify(
    bytes: &[u8],
    info: &ScreenshotInfo,
    options: &ScreenshotOptions,
) -> Result<(), String> {
    info.validate(options).map_err(str::to_owned)?;
    if bytes.len() > options.byte_limit()
        || dimensions(bytes, info.format)? != (info.width, info.height)
    {
        return Err("screenshot dimensions/bytes do not match metadata and budgets".into());
    }
    let complete = match info.format {
        ScreenshotFormat::Png => bytes.ends_with(b"\x00\x00\x00\x00IEND\xae\x42\x60\x82"),
        ScreenshotFormat::Jpeg => bytes.ends_with(b"\xff\xd9"),
    };
    if !complete {
        return Err("screenshot stream is truncated or has trailing data".into());
    }
    // Validate the actual codec stream, including corrupt/truncated data behind a plausible header.
    let decoded = reader(bytes, info.format)
        .decode()
        .map_err(|e| e.to_string())?;
    if (decoded.width(), decoded.height()) != (info.width, info.height) {
        return Err("decoded screenshot dimensions mismatch".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn noisy(w: u32, h: u32) -> DynamicImage {
        let mut seed = 17u32;
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |_, _| {
            let mut pixel = [0u8; 3];
            for value in &mut pixel {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *value = seed as u8;
            }
            image::Rgb(pixel)
        }))
    }
    #[test]
    fn noisy_preview_adapts_quality_and_dimensions_to_budget() {
        let options = ScreenshotOptions {
            max_bytes: Some(16384),
            max_width: Some(640),
            max_height: Some(400),
            ..Default::default()
        };
        let image = encode(noisy(960, 600), &options, Some(7), (-1920, 0)).unwrap();
        assert!(image.bytes.len() <= 16384);
        assert!(image.info.width <= 640 && image.info.height <= 400);
        assert_eq!(
            (image.info.source_width, image.info.source_height),
            (960, 600)
        );
        assert!(image.info.resized);
        assert_eq!(image.info.monitor_id, Some(7));
        assert_eq!(image.info.origin_x, -1920);
        verify(&image.bytes, &image.info, &options).unwrap();
    }
    #[test]
    fn original_png_never_downscales_or_silently_changes_format() {
        let options = ScreenshotOptions {
            max_bytes: Some(16384),
            ..ScreenshotOptions::original()
        };
        assert!(
            encode(noisy(512, 512), &options, None, (0, 0))
                .unwrap_err()
                .contains("original encoding")
        );
        let options = ScreenshotOptions::original();
        let image = encode(noisy(120, 80), &options, None, (0, 0)).unwrap();
        assert_eq!((image.info.width, image.info.height), (120, 80));
        assert_eq!(image.info.format, ScreenshotFormat::Png);
        assert!(!image.info.resized);
        verify(&image.bytes, &image.info, &options).unwrap();
    }
    #[test]
    fn verification_rejects_truncation_wrong_format_and_invented_metadata() {
        for options in [ScreenshotOptions::default(), ScreenshotOptions::original()] {
            let image = encode(noisy(120, 80), &options, None, (0, 0)).unwrap();
            assert!(verify(&image.bytes[..image.bytes.len() - 2], &image.info, &options).is_err());
            let mut corrupted = image.bytes.clone();
            corrupted.truncate(corrupted.len() / 2);
            assert!(verify(&corrupted, &image.info, &options).is_err());
            let other = if image.info.format == ScreenshotFormat::Png {
                ScreenshotFormat::Jpeg
            } else {
                ScreenshotFormat::Png
            };
            assert!(dimensions(&image.bytes, other).is_err());
            let mut wrong = image.info.clone();
            wrong.width += 1;
            assert!(verify(&image.bytes, &wrong, &options).is_err());
        }
    }
    #[test]
    fn selected_region_and_original_jpeg_quality_must_match() {
        let options = ScreenshotOptions {
            mode: ScreenshotMode::Original,
            format: Some(ScreenshotFormat::Jpeg),
            quality: Some(90),
            monitor_id: Some(12),
            region: Some(ScreenshotRegion {
                x: 50,
                y: 20,
                width: 120,
                height: 80,
            }),
            ..Default::default()
        };
        assert!(encode(noisy(120, 79), &options, Some(12), (50, 20)).is_err());
        assert!(encode(noisy(120, 80), &options, Some(13), (50, 20)).is_err());
        let image = encode(noisy(120, 80), &options, Some(12), (50, 20)).unwrap();
        assert_eq!(image.info.quality, Some(90));
        verify(&image.bytes, &image.info, &options).unwrap();
    }
}
