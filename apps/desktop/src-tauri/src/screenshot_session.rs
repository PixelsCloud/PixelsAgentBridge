#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub fn capture_png() -> Result<Vec<u8>, String> {
    use pab_screenshot::image::DynamicImage;
    use xcap::Monitor;

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
    pab_screenshot::encode(
        DynamicImage::ImageRgba8(image),
        &pab_protocol::ScreenshotOptions::original(),
        None,
        (0, 0),
    )
    .map(|image| image.bytes)
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub fn capture(
    options: &pab_protocol::ScreenshotOptions,
) -> Result<pab_screenshot::EncodedScreenshot, String> {
    use pab_screenshot::image::DynamicImage;
    use xcap::Monitor;
    options.validate().map_err(str::to_owned)?;
    if cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return Err("Wayland screenshot selection is not supported by this PAB interface".into());
    }
    let monitors = Monitor::all().map_err(|e| e.to_string())?;
    let monitor = if let Some(id) = options.monitor_id {
        monitors
            .iter()
            .find(|m| m.id().ok() == Some(id))
            .ok_or("selected monitor is unavailable; it may have been unplugged")?
    } else {
        monitors
            .iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .ok_or("no primary interactive monitor is available")?
    };
    let id = monitor.id().map_err(|e| e.to_string())?;
    let (w, h) = (
        monitor.width().map_err(|e| e.to_string())?,
        monitor.height().map_err(|e| e.to_string())?,
    );
    let (x, y) = (
        monitor.x().map_err(|e| e.to_string())?,
        monitor.y().map_err(|e| e.to_string())?,
    );
    let captured_at_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    let (image, origin, expected) = if let Some(r) = &options.region {
        if r.x.checked_add(r.width).is_none_or(|end| end > w)
            || r.y.checked_add(r.height).is_none_or(|end| end > h)
        {
            return Err("region is outside the selected monitor; no implicit clipping".into());
        }
        let ox = x
            .checked_add(i32::try_from(r.x).map_err(|_| "region coordinate overflow")?)
            .ok_or("region coordinate overflow")?;
        let oy = y
            .checked_add(i32::try_from(r.y).map_err(|_| "region coordinate overflow")?)
            .ok_or("region coordinate overflow")?;
        (
            monitor
                .capture_region(r.x, r.y, r.width, r.height)
                .map_err(|e| e.to_string())?,
            (ox, oy),
            (r.width, r.height),
        )
    } else {
        if w == 0
            || h == 0
            || w > 16384
            || h > 16384
            || u64::from(w) * u64::from(h) > pab_protocol::MAX_SCREENSHOT_PIXELS
        {
            return Err("monitor exceeds capture pixel budget; choose a bounded region".into());
        }
        (
            monitor.capture_image().map_err(|e| e.to_string())?,
            (x, y),
            (w, h),
        )
    };
    if (image.width(), image.height()) != expected {
        return Err("captured dimensions changed during monitor selection; retry with fresh monitor information".into());
    }
    let mut encoded =
        pab_screenshot::encode(DynamicImage::ImageRgba8(image), options, Some(id), origin)?;
    encoded.info.captured_at_unix_ms = captured_at_unix_ms;
    Ok(encoded)
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub fn capture(
    _: &pab_protocol::ScreenshotOptions,
) -> Result<pab_screenshot::EncodedScreenshot, String> {
    Err("interactive screenshots are not supported on this platform".into())
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub fn capture_png() -> Result<Vec<u8>, String> {
    Err("interactive screenshots are not supported on this platform".to_owned())
}
