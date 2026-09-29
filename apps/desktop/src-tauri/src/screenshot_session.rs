#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub fn capture_png() -> Result<Vec<u8>, String> {
    use std::io::Cursor;
    use xcap::{
        Monitor,
        image::{DynamicImage, ImageFormat},
    };

    let monitors = Monitor::all().map_err(|error| error.to_string())?;
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())
        .ok_or_else(|| "no interactive monitor is available".to_owned())?;
    let width = monitor.width().map_err(|error| error.to_string())?;
    let height = monitor.height().map_err(|error| error.to_string())?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > pab_protocol::MAX_SCREENSHOT_PIXELS {
        return Err("screen dimensions exceed the supported limit".to_owned());
    }
    let image = monitor.capture_image().map_err(|error| error.to_string())?;
    if image.width() != width || image.height() != height {
        return Err("captured image dimensions differ from the selected monitor".to_owned());
    }
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(image)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    if bytes.len() > pab_protocol::MAX_SCREENSHOT_BYTES {
        return Err("screenshot exceeds the supported size".to_owned());
    }
    Ok(bytes)
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub fn capture_png() -> Result<Vec<u8>, String> {
    Err("interactive screenshots are not supported on this platform".to_owned())
}
