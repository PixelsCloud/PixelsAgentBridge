//! Shared screenshot wire and encoder; capture still uses Android's consented backend.
use crate::{
    broker::{PlatformBroker, ReferenceScope},
    system::SystemService,
};
use pab_protocol::*;
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

pub(crate) async fn capture(
    broker: &Arc<PlatformBroker>,
    system: &SystemService,
    owner: &str,
    scope: &str,
    references: Arc<Mutex<ReferenceScope>>,
    id: RequestId,
    options: ScreenshotOptions,
) -> Result<(ScreenshotMeta, Vec<u8>), String> {
    let permit = system.capture_permit()?;
    options.validate().map_err(str::to_owned)?;
    if options.mode == ScreenshotMode::Original {
        return Err(
            "Android capture supplies a JPEG frame; original lossless capture is unavailable"
                .into(),
        );
    }
    if options.monitor_id.is_some_and(|id| id != 0) {
        return Err("Android exposes only the active display (monitor_id 0)".into());
    }
    let before = system
        .capture_context(owner, scope, id, options.window_ref.as_deref())
        .await
        .map_err(str::to_owned)?;
    let mut reply = broker
        .request(
            owner,
            references.clone(),
            MobileRequest::Execute {
                request_id: id,
                command: MobileCommand::Screenshot {},
            },
        )
        .await;
    let until = tokio::time::Instant::now() + Duration::from_secs(30);
    while reply.state == MobileState::Running {
        if tokio::time::Instant::now() >= until {
            return Err("screenshot timed out; original request is not replayed".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        reply = broker
            .request(
                owner,
                references.clone(),
                MobileRequest::Get { request_id: id },
            )
            .await;
    }
    if reply.state != MobileState::Completed {
        return Err(reply
            .error
            .unwrap_or_else(|| "screenshot_unavailable".into()));
    }
    let bytes = broker
        .image(owner, id)
        .await
        .ok_or("screenshot image expired; original not replayed")?;
    let after = system
        .capture_context(owner, scope, id, options.window_ref.as_deref())
        .await
        .map_err(str::to_owned)?;
    if before != after {
        return Err("window or display changed during capture".into());
    }
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        use pab_screenshot::image::{ImageReader, Limits};
        let mut reader = ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let mut limits = Limits::default();
        limits.max_image_width = Some(16384);
        limits.max_image_height = Some(16384);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        let mut image = reader.decode().map_err(|e| e.to_string())?;
        if u64::from(image.width()) * u64::from(image.height()) > MAX_SCREENSHOT_PIXELS {
            return Err("capture pixel limit".into());
        }
        if before["width"].as_u64() != Some(image.width().into())
            || before["height"].as_u64() != Some(image.height().into())
        {
            return Err("display geometry changed during capture".into());
        }
        let mut origin = (0, 0);
        if let Some(region) = &options.region {
            if region.x + region.width > image.width() || region.y + region.height > image.height()
            {
                return Err("screenshot region outside Android display".into());
            }
            image = image.crop_imm(region.x, region.y, region.width, region.height);
            origin = (region.x as i32, region.y as i32);
        }
        let mut encoder_options = options.clone();
        encoder_options.window_ref = None;
        let mut encoded = pab_screenshot::encode(image, &encoder_options, Some(0), origin)?;
        encoded.info.window_ref = options.window_ref.clone();
        encoded.info.desktop_rect = Some(ScreenshotDesktopRect {
            x: origin.0,
            y: origin.1,
            width: encoded.info.source_width,
            height: encoded.info.source_height,
        });
        encoded.info.validate(&options).map_err(str::to_owned)?;
        let meta = ScreenshotMeta {
            request_id: id,
            format: encoded.info.format.name().into(),
            width: encoded.info.width,
            height: encoded.info.height,
            size: encoded.bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&encoded.bytes)),
            capture: Some(encoded.info),
        };
        Ok((meta, encoded.bytes))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
#[path = "screenshot_tests.rs"]
mod tests;
