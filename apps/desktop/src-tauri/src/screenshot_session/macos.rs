use pab_protocol::{ScreenshotMode, ScreenshotOptions};
use pab_screenshot::{EncodedScreenshot, image::DynamicImage};

pub fn capture_png() -> Result<Vec<u8>, String> {
    capture(&ScreenshotOptions::original()).map(|encoded| encoded.bytes)
}

pub fn capture(options: &ScreenshotOptions) -> Result<EncodedScreenshot, String> {
    options.validate().map_err(str::to_owned)?;
    if !pab_desktop_control::active_console() {
        return Err("macOS capture requires the active console user".into());
    }
    pab_desktop_control::require_screen_capture()?;
    if options.window_ref.is_some() {
        return Err("window capture requires its referenced desktop session".into());
    }
    let monitors = xcap::Monitor::all().map_err(|e| e.to_string())?;
    let monitor = monitors
        .iter()
        .find(|m| match options.monitor_id {
            Some(id) => m.id().ok() == Some(id),
            None => m.is_primary().unwrap_or(false),
        })
        .ok_or("selected monitor is unavailable")?;
    let id = monitor.id().map_err(|e| e.to_string())?;
    let scale = monitor.scale_factor().map_err(|e| e.to_string())?;
    if !scale.is_finite() || !(1.0..=8.0).contains(&scale) {
        return Err("invalid monitor scale".into());
    }
    let w = monitor.width().map_err(|e| e.to_string())?;
    let h = monitor.height().map_err(|e| e.to_string())?;
    let x = monitor.x().map_err(|e| e.to_string())?;
    let y = monitor.y().map_err(|e| e.to_string())?;
    if w == 0
        || h == 0
        || (options.mode != ScreenshotMode::Jpeg
            && f64::from(w) * f64::from(h) * f64::from(scale).powi(2)
                > pab_protocol::MAX_SCREENSHOT_PIXELS as f64)
    {
        return Err("monitor exceeds capture pixel budget".into());
    }
    let mut image = monitor.capture_image().map_err(|e| e.to_string())?;
    if monitor.width().ok() != Some(w)
        || monitor.height().ok() != Some(h)
        || monitor.scale_factor().ok() != Some(scale)
    {
        return Err("monitor geometry changed during capture; retry".into());
    }
    let mut origin = (x, y);
    let mut desktop_size = (w, h);
    // Preserve the protocol's region unit (image pixels) and original dimensions.
    // Cropping a logical CG region would return scale-times larger pixels on Retina.
    if let Some(region) = &options.region {
        if region
            .x
            .checked_add(region.width)
            .is_none_or(|end| end > image.width())
            || region
                .y
                .checked_add(region.height)
                .is_none_or(|end| end > image.height())
        {
            return Err("region is outside captured pixel bounds".into());
        }
        let sx = f64::from(image.width()) / f64::from(w);
        let sy = f64::from(image.height()) / f64::from(h);
        origin.0 = x
            .checked_add((f64::from(region.x) / sx).round() as i32)
            .ok_or("coordinate overflow")?;
        origin.1 = y
            .checked_add((f64::from(region.y) / sy).round() as i32)
            .ok_or("coordinate overflow")?;
        desktop_size = (
            (f64::from(region.width) / sx).round().max(1.0) as u32,
            (f64::from(region.height) / sy).round().max(1.0) as u32,
        );
        image = pab_screenshot::image::imageops::crop_imm(
            &image,
            region.x,
            region.y,
            region.width,
            region.height,
        )
        .to_image();
    }
    let mut encoded =
        pab_screenshot::encode(DynamicImage::ImageRgba8(image), options, Some(id), origin)?;
    // The existing optional rectangle lets every updated MCP host map Retina image
    // pixels to desktop points without guessing a scale from its own platform.
    encoded.info.desktop_rect = Some(pab_protocol::ScreenshotDesktopRect {
        x: origin.0,
        y: origin.1,
        width: desktop_size.0,
        height: desktop_size.1,
    });
    encoded.info.validate(options).map_err(str::to_owned)?;
    Ok(encoded)
}
