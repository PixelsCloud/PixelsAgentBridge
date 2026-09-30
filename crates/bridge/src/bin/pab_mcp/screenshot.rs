//! Image payloads stay in MCP content, never in JSON metadata or presence reports.
use base64::{Engine, engine::general_purpose::STANDARD};
use pab_bridge::{BridgeRuntime, Screenshot};
use pab_protocol::{MAX_INLINE_SCREENSHOT_BYTES, ScreenshotOptions};
use rmcp::model::ContentBlock;
use serde_json::{Value, json};

pub async fn call(
    runtime: &BridgeRuntime,
    args: &Value,
) -> Result<(Value, Option<ContentBlock>), String> {
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
fn response(image: Screenshot, include_image: bool) -> (Value, Option<ContentBlock>) {
    let fits = image.bytes.len() <= MAX_INLINE_SCREENSHOT_BYTES;
    let mime = if image.meta.format == "jpeg" {
        "image/jpeg"
    } else {
        "image/png"
    };
    let content =
        (include_image && fits).then(|| ContentBlock::image(STANDARD.encode(&image.bytes), mime));
    let mut metadata = serde_json::to_value(image.meta).expect("ScreenshotMeta is serializable");
    metadata["image_included"] = json!(content.is_some());
    metadata["inline_max_bytes"] = json!(MAX_INLINE_SCREENSHOT_BYTES);
    if !include_image {
        metadata["image_omitted_reason"] =
            json!("include_image is false; image is saved to destination");
    } else if !fits {
        metadata["image_omitted_reason"] = json!(
            "encoded image exceeds inline_max_bytes; image is saved to destination; use preview for inline display"
        );
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
    fn inline_byte_limit_and_explicit_file_only_are_honored() {
        assert!(
            response(fixture(MAX_INLINE_SCREENSHOT_BYTES), true)
                .1
                .is_some()
        );
        let (meta, image) = response(fixture(MAX_INLINE_SCREENSHOT_BYTES + 1), true);
        assert!(image.is_none());
        assert_eq!(meta["image_included"], false);
        assert!(meta["image_omitted_reason"].is_string());
        assert!(response(fixture(100), false).1.is_none());
    }
}
