use image::{ImageDecoder, ImageEncoder};
use pab_protocol::*;
use std::io::{self, Cursor, Write};

pub(super) fn error(message: &'static str) -> io::Error {
    io::Error::other(message)
}
fn limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(DESKTOP_CLIPBOARD_RGBA_BYTES as u64);
    limits
}
pub(super) fn decode(image: &DesktopClipboardImage) -> io::Result<image::RgbaImage> {
    image.validate().map_err(error)?;
    let mut reader =
        image::ImageReader::with_format(Cursor::new(&image.data), image::ImageFormat::Png);
    reader.limits(limits());
    reader
        .decode()
        .map(|v| v.to_rgba8())
        .map_err(|_| error("clipboard_image_invalid"))
}
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > DESKTOP_CLIPBOARD_IMAGE_BYTES.saturating_sub(self.0.len()) {
            return Err(error("clipboard_image_too_large"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(super) fn encode(width: u32, height: u32, rgba: &[u8]) -> io::Result<DesktopClipboardImage> {
    validate_clipboard_dimensions(width, height).map_err(error)?;
    if rgba.len() as u64 != u64::from(width) * u64::from(height) * 4 {
        return Err(error("clipboard_image_invalid"));
    }
    let mut output = Bounded(Vec::new());
    image::codecs::png::PngEncoder::new_with_quality(
        &mut output,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    )
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|_| error("clipboard_image_encode_failed"))?;
    Ok(DesktopClipboardImage {
        width,
        height,
        bytes: output.0.len() as u32,
        data: output.0,
    })
}
#[cfg(windows)]
pub(super) fn optimize_png(image: DesktopClipboardImage) -> io::Result<DesktopClipboardImage> {
    let rgba = decode(&image)?;
    // Native PNGs may already be smaller (e.g. indexed images). Preserve the
    // original if recompression grows it or hits the encoded-size limit.
    // Keep higher precision and color-managed PNGs intact: the RGBA8 clipboard
    // encoder must not discard their precision, gamma, profile or orientation.
    let mut decoder = image::codecs::png::PngDecoder::with_limits(Cursor::new(&image.data), limits())
        .map_err(|_| error("clipboard_image_invalid"))?;
    if image.data[24] != 8
        || decoder.gamma_value().map_err(|_| error("clipboard_image_invalid"))?.is_some()
        || decoder.icc_profile().map_err(|_| error("clipboard_image_invalid"))?.is_some()
        || decoder.exif_metadata().map_err(|_| error("clipboard_image_invalid"))?.is_some()
    {
        return Ok(image);
    }
    match encode(image.width, image.height, rgba.as_raw()) {
        Ok(smaller) if smaller.bytes < image.bytes => Ok(smaller),
        _ => Ok(image),
    }
}
#[cfg(windows)]
pub(super) fn from_dib(data: Vec<u8>) -> io::Result<DesktopClipboardImage> {
    if data.len() < 40 {
        return Err(error("clipboard_image_invalid"));
    }
    let width = i32::from_le_bytes(data[4..8].try_into().unwrap());
    let height = i32::from_le_bytes(data[8..12].try_into().unwrap())
        .checked_abs()
        .ok_or_else(|| error("clipboard_image_invalid"))?;
    if width <= 0 || height <= 0 {
        return Err(error("clipboard_image_invalid"));
    }
    validate_clipboard_dimensions(width as u32, height as u32).map_err(error)?;
    let mut decoder = image::codecs::bmp::BmpDecoder::new_without_file_header(Cursor::new(data))
        .map_err(|_| error("clipboard_image_invalid"))?;
    decoder
        .set_limits(limits())
        .map_err(|_| error("clipboard_image_too_large"))?;
    let decoded = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| error("clipboard_image_invalid"))?
        .to_rgba8();
    encode(decoded.width(), decoded.height(), decoded.as_raw())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strong_png_compression_preserves_full_rgba() {
        let (width,height)=(1920u32,1080u32);
        let mut rgba=Vec::with_capacity((width*height*4) as usize);
        for y in 0..height {for x in 0..width {
            rgba.extend_from_slice(&[(x%256) as u8,(y%256) as u8,((x+y)%256) as u8,if x<width/2{255}else{128}]);
        }}
        let mut fast=Vec::new();
        let started=std::time::Instant::now();
        image::codecs::png::PngEncoder::new(&mut fast)
            .write_image(&rgba,width,height,image::ExtendedColorType::Rgba8).unwrap();
        let fast_ms=started.elapsed().as_millis();
        let started=std::time::Instant::now();
        let compressed=encode(width,height,&rgba).unwrap();
        let strong_ms=started.elapsed().as_millis();
        assert!(compressed.data.len()<fast.len());
        assert_eq!(decode(&compressed).unwrap().as_raw(),&rgba);
        println!("synthetic_1080p fast_bytes={} strong_bytes={} fast_ms={} strong_ms={}",fast.len(),compressed.bytes,fast_ms,strong_ms);
        #[cfg(windows)] {
            let native=DesktopClipboardImage{width,height,bytes:fast.len() as u32,data:fast};
            let optimized=optimize_png(native).unwrap();
            assert_eq!(optimized,compressed);
            let again=optimize_png(optimized.clone()).unwrap();
            assert_eq!(again,optimized);
        }
    }
    #[test]
    #[cfg(windows)]
    fn native_png_keeps_smaller_original_and_high_precision() {
        let mut tiny=Vec::new();
        image::codecs::png::PngEncoder::new_with_quality(
            &mut tiny,
            image::codecs::png::CompressionType::Best,
            image::codecs::png::FilterType::Adaptive,
        )
            .write_image(&[255],1,1,image::ExtendedColorType::L8).unwrap();
        let original=DesktopClipboardImage{width:1,height:1,bytes:tiny.len() as u32,data:tiny};
        assert_eq!(optimize_png(original.clone()).unwrap(),original);
        let mut precise=Vec::new();
        image::codecs::png::PngEncoder::new(&mut precise)
            .write_image(&12345u16.to_ne_bytes(),1,1,image::ExtendedColorType::L16).unwrap();
        let original=DesktopClipboardImage{width:1,height:1,bytes:precise.len() as u32,data:precise};
        assert_eq!(optimize_png(original.clone()).unwrap(),original);
    }
    #[test]
    #[cfg(windows)]
    fn dib_screenshot_bottom_up_bgr_and_top_down_reject_truncated() {
        let mut dib = vec![0u8; 48];
        dib[..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&2i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&24u16.to_le_bytes());
        dib[40..46].copy_from_slice(&[0, 0, 255, 0, 255, 0]);
        assert_eq!(
            decode(&from_dib(dib.clone()).unwrap()).unwrap().as_raw(),
            &[255, 0, 0, 255, 0, 255, 0, 255]
        );
        dib[8..12].copy_from_slice(&(-1i32).to_le_bytes());
        assert!(from_dib(dib.clone()).is_ok());
        dib.truncate(42);
        assert!(from_dib(dib).is_err());
    }
    #[test]
    fn png_preserves_transparency_colors_and_rejects_headers_before_decode() {
        let pixels = [
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 200, 100, 50, 255,
        ];
        let image = encode(2, 2, &pixels).unwrap();
        assert_eq!(decode(&image).unwrap().as_raw(), &pixels);
        let mut bad = image.clone();
        bad.width = 16384;
        bad.height = 16384;
        assert!(decode(&bad).is_err());
        let mut bad = image.clone();
        bad.data.truncate(24);
        assert!(decode(&bad).is_err());
        let mut bad = image;
        bad.data[40] ^= 1;
        assert!(decode(&bad).is_err());
    }
}
