//! Image payloads stay in MCP content, never in JSON metadata or presence reports.
use base64::{Engine, engine::general_purpose::STANDARD};
use pab_bridge::{BridgeRuntime, Screenshot};
use pab_protocol::ScreenshotOptions;
use rmcp::model::ContentBlock;
use serde_json::{Value, json};

pub async fn call(
    runtime: &BridgeRuntime,
    args: &Value,
) -> Result<(Value, Option<ContentBlock>), String> {
    let options = parse_options(args)?;
    let destination = args
        .get("destination")
        .map(|v| v.as_str().ok_or("destination must be a string"))
        .transpose()?;
    let device = super::mcp_tools::resolve_target(runtime, args).await?;
    let target = runtime
        .current_environment(device)
        .await
        .map_err(|e| e.to_string())?;
    let image = runtime
        .capture_screenshot_with_options(device, destination.map(std::path::Path::new), &options)
        .await
        .map_err(|e| e.to_string())?;
    let archive = runtime.screenshot_archive_path(image.meta.request_id, options.format());
    let (mut metadata, content) = response(
        image,
        args.get("include_image")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    );
    metadata["destination"] = json!(
        destination
            .map(str::to_owned)
            .unwrap_or_else(|| archive.to_string_lossy().into_owned())
    );
    metadata["os_reminder"] = json!(target.compact_reminder());
    Ok((metadata, content))
}
fn parse_options(args: &Value) -> Result<ScreenshotOptions, String> {
    let mut capture_args = args
        .as_object()
        .ok_or("arguments must be an object")?
        .clone();
    for field in ["device_code", "destination", "include_image"] {
        capture_args.remove(field);
    }
    let options: ScreenshotOptions =
        serde_json::from_value(Value::Object(capture_args)).map_err(|e| e.to_string())?;
    options.validate().map_err(str::to_owned)?;
    if options.mode != pab_protocol::ScreenshotMode::Jpeg {
        return Err("screenshots preserve captured resolution and return JPEG".into());
    }
    Ok(options)
}
fn response(image: Screenshot, include_image: bool) -> (Value, Option<ContentBlock>) {
    let mime = if image.meta.format == "jpeg" {
        "image/jpeg"
    } else {
        "image/png"
    };
    let content = include_image.then(|| ContentBlock::image(STANDARD.encode(&image.bytes), mime));
    let mapping=image.meta.capture.as_ref().map(|info| {
        let (w,h)=info.desktop_rect.as_ref().map(|r|(r.width,r.height)).unwrap_or((info.source_width,info.source_height));
        json!({"coordinate_space":"xcap_native", "origin_x":info.origin_x,"origin_y":info.origin_y,
            "scale_x":f64::from(w)/f64::from(info.width),"scale_y":f64::from(h)/f64::from(info.height),
            "formula":"desktop = origin + preview_pixel * scale; mapping describes capture time, recheck window before input"})
    });
    let mut metadata = serde_json::to_value(image.meta).expect("ScreenshotMeta is serializable");
    metadata["preview_to_desktop"] = json!(mapping);
    metadata["image_included"] = json!(content.is_some());
    if !include_image {
        metadata["image_omitted_reason"] =
            json!("include_image is false; image is saved to destination");
    }
    (metadata, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_protocol::{RequestId, ScreenshotMeta};
    fn fixture(size: usize) -> Screenshot {
        Screenshot {
            bytes: vec![42; size],
            meta: ScreenshotMeta {
                request_id: RequestId::new(),
                format: "jpeg".into(),
                width: 100,
                height: 100,
                size: size as u64,
                sha256: "a".repeat(64),
                capture: None,
            },
        }
    }
    #[test]
    fn screenshot_entry_uses_native_jpeg_and_rejects_resize_options() {
        assert!(parse_options(&json!({})).is_ok());
        assert!(parse_options(&json!({"mode":"original"})).is_err());
        assert!(parse_options(&json!({"max_bytes":524289})).is_err());
        assert!(parse_options(&json!({"max_width":1600})).is_err());
        assert!(parse_options(&json!({"max_height":1000})).is_err());
        assert!(parse_options(&json!({"mode":"preview"})).is_err());
        assert_eq!(
            parse_options(&json!({})).unwrap().mode,
            pab_protocol::ScreenshotMode::Jpeg
        );
        let reference = RequestId::new().to_string();
        assert!(parse_options(&json!({"window_ref":reference})).is_ok());
        assert!(parse_options(&json!({"window_ref":reference,"monitor_id":1})).is_err());
        assert!(
            super::super::mcp_catalog::validate_arguments(
                "pab_capture_screenshot",
                &json!({"device_code":"123456789","mode":"original"})
            )
            .is_err()
        );
    }
    #[test]
    fn mapping_uses_window_coordinate_extent_instead_of_dpi_scaled_source_pixels() {
        use pab_protocol::*;
        let mut image = fixture(500);
        image.meta.width = 800;
        image.meta.height = 600;
        image.meta.capture = Some(ScreenshotInfo {
            window_ref: Some(RequestId::new().to_string()),
            desktop_rect: Some(ScreenshotDesktopRect {
                x: -800,
                y: 30,
                width: 400,
                height: 300,
            }),
            captured_at_unix_ms: 1,
            mode: ScreenshotMode::Jpeg,
            format: ScreenshotFormat::Jpeg,
            width: 800,
            height: 600,
            source_width: 800,
            source_height: 600,
            monitor_id: Some(1),
            origin_x: -800,
            origin_y: 30,
            quality: Some(85),
            resized: false,
        });
        let (meta, content) = response(image, true);
        assert!(content.is_some());
        assert_eq!(meta["preview_to_desktop"]["scale_x"], 0.5);
        assert_eq!(meta["preview_to_desktop"]["scale_y"], 0.5);
        assert_eq!(meta["preview_to_desktop"]["origin_x"], -800);
        assert!(meta.get("data").is_none());
    }
    #[test]
    fn retina_monitor_mapping_keeps_pixels_and_logical_coordinates_distinct() {
        use pab_protocol::*;
        let mut image = fixture(500);
        let info = ScreenshotInfo {
            window_ref: None,
            desktop_rect: Some(ScreenshotDesktopRect {
                x: -1280,
                y: 0,
                width: 1280,
                height: 800,
            }),
            captured_at_unix_ms: 1,
            mode: ScreenshotMode::Jpeg,
            format: ScreenshotFormat::Jpeg,
            width: 2560,
            height: 1600,
            source_width: 2560,
            source_height: 1600,
            monitor_id: Some(2),
            origin_x: -1280,
            origin_y: 0,
            quality: Some(85),
            resized: false,
        };
        info.validate(&ScreenshotOptions::default()).unwrap();
        image.meta.capture = Some(info.clone());
        let (meta, _) = response(image, false);
        assert_eq!(meta["preview_to_desktop"]["scale_x"], 0.5);
        let mut invalid = info;
        invalid.monitor_id = None;
        assert!(invalid.validate(&ScreenshotOptions::default()).is_err());
        invalid.monitor_id = Some(2);
        invalid.origin_x = 0;
        assert!(invalid.validate(&ScreenshotOptions::default()).is_err());
    }
    #[test]
    fn cropped_image_maps_through_native_origin_before_monitor_scaling() {
        use pab_protocol::*;
        let mut image = fixture(500);
        image.meta.capture = Some(ScreenshotInfo {
            window_ref: None,
            desktop_rect: Some(ScreenshotDesktopRect {
                x: -1770,
                y: 300,
                width: 600,
                height: 300,
            }),
            captured_at_unix_ms: 1,
            mode: ScreenshotMode::Jpeg,
            format: ScreenshotFormat::Jpeg,
            width: 600,
            height: 300,
            source_width: 600,
            source_height: 300,
            monitor_id: Some(1),
            origin_x: -1770,
            origin_y: 300,
            quality: Some(85),
            resized: false,
        });
        let (meta, _) = response(image, false);
        let mapping = &meta["preview_to_desktop"];
        // Crop starts 150 native pixels into a 150%-scaled monitor at (-1920,0).
        // Image point (150,150) is native (-1620,450), monitor logical (200,300).
        assert_eq!(
            mapping["origin_x"].as_f64().unwrap() + 150.0 * mapping["scale_x"].as_f64().unwrap(),
            -1620.0
        );
        assert_eq!(
            mapping["origin_y"].as_f64().unwrap() + 150.0 * mapping["scale_y"].as_f64().unwrap(),
            450.0
        );
        let target = MonitorTarget {
            helper_instance: RequestId::new().to_string(),
            id: 1,
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
            scale_percent: 150,
            rotation_degrees: 0,
            coordinate_space: MonitorCoordinateSpace::PhysicalPixels,
        };
        assert_eq!(target.native_point(200, 300).unwrap(), (-1620, 450));
    }
    #[test]
    fn image_is_present_only_in_image_content_not_json_or_text() {
        let (meta, content) = response(fixture(500), true);
        assert_eq!(meta["image_included"], true);
        assert!(meta.get("data").is_none());
        let mut result = rmcp::model::CallToolResult::structured(meta);
        result.content.push(content.unwrap());
        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["content"][1]["type"], "image");
        assert_eq!(json["content"][1]["mimeType"], "image/jpeg");
        assert_eq!(
            STANDARD
                .decode(json["content"][1]["data"].as_str().unwrap())
                .unwrap(),
            vec![42; 500]
        );
        assert!(json["content"][0]["text"].as_str().unwrap().len() < 1000);
        assert!(json["structuredContent"].get("data").is_none());
    }
    #[test]
    fn full_image_is_returned_above_old_limits_unless_file_only_is_requested() {
        for size in [512 * 1024 + 1, 8 * 1024 * 1024 + 1] {
            let (meta, image) = response(fixture(size), true);
            assert_eq!(meta["image_included"], true);
            assert!(meta.get("image_omitted_reason").is_none());
            let value = serde_json::to_value(image.unwrap()).unwrap();
            assert_eq!(
                STANDARD.decode(value["data"].as_str().unwrap()).unwrap(),
                vec![42; size]
            );
        }
        assert!(response(fixture(100), false).1.is_none());
    }
}
