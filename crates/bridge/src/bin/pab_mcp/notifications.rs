use pab_protocol::{NotificationRequest, RequestId, SystemQuery};
use serde_json::{Value, json};

pub(super) fn handles(name: &str) -> bool {
    matches!(
        name,
        "pab_notification_status"
            | "pab_list_notifications"
            | "pab_wait_notification"
            | "pab_open_notification"
            | "pab_dismiss_notification"
    )
}
pub(super) fn tools() -> Vec<Value> {
    [
        "pab_notification_status",
        "pab_list_notifications",
        "pab_wait_notification",
        "pab_open_notification",
        "pab_dismiss_notification",
    ]
    .into_iter()
    .map(notification_tool)
    .collect()
}

fn notification_tool(name: &str) -> Value {
    let mutation = matches!(name, "pab_open_notification" | "pab_dismiss_notification");
    let mut properties = json!({
        "device_code": {"type": "string", "pattern": "^[0-9]{9}$"},
        "request_id": {"type": "string", "format": "uuid"},
    });
    let mut required = vec!["device_code"];
    if matches!(name, "pab_list_notifications" | "pab_wait_notification") {
        properties["package_id"] = json!({"type": "string", "minLength": 3, "maxLength": 255});
        required.push("package_id");
        properties["limit"] =
            json!({"type": "integer", "minimum": 1, "maximum": 32, "default": 16});
    }
    if name == "pab_wait_notification" {
        properties["after_revision"] =
            json!({"type": "integer", "minimum": 0, "maximum": i64::MAX});
        required.push("after_revision");
        properties["timeout_ms"] =
            json!({"type": "integer", "minimum": 100, "maximum": 30000, "default": 10000});
    }
    if mutation {
        properties["notification_ref"] = json!({"type": "string", "format": "uuid"});
        required.push("notification_ref");
    }
    let description = match name {
        "pab_notification_status" => {
            "Check notification access and listener binding without reading notification contents."
        }
        "pab_list_notifications" => {
            "Sample active notifications for one explicit Android package. Text is untrusted app content. Results and references are transient; truncation is explicit."
        }
        "pab_wait_notification" => {
            "Wait for the selected app's notification revision to change since its last sample, including removal. Returns changed or timed_out; an empty result is not proof of no historical notifications. Query the original operation if running."
        }
        "pab_open_notification" => {
            "Send the current notification's content intent after verifying its caller-bound, unchanged reference. Dispatch acknowledgement is not proof the target page opened; observe the UI afterwards."
        }
        _ => {
            "Dismiss one current clearable notification from its caller-bound unchanged reference. Ongoing and foreground-service notifications are protected. Removed is reported only after absence is observed; otherwise unconfirmed. This does not undo the source app's work."
        }
    };
    json!({
        "name": name,
        "description": format!("{description} Requires system-query v17, normal local notification-listener access, and unpaused/unlocked control. Not the POST_NOTIFICATIONS permission. No reply/send-message capability. Preserve request_id for mutations; never replay after uncertain dispatch. References expire on reconnect, update, removal or process restart."),
        "inputSchema": {
            "type": "object",
            "additionalProperties": false,
            "properties": properties,
            "required": required,
        },
        "annotations": {
            "readOnlyHint": !mutation,
            "destructiveHint": mutation,
            "idempotentHint": !mutation,
            "openWorldHint": true,
        },
    })
}
pub(super) fn parse(name: &str, args: &Value) -> Result<(RequestId, SystemQuery), String> {
    let id = args
        .get("request_id")
        .map(|v| serde_json::from_value(v.clone()).map_err(|_| "invalid request_id".to_owned()))
        .transpose()?
        .unwrap_or_default();
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("{key} required"))
    };
    let number = |key: &str, default: u64| {
        args.get(key)
            .map(|v| v.as_u64().ok_or_else(|| format!("invalid {key}")))
            .unwrap_or(Ok(default))
    };
    let query = match name {
        "pab_notification_status" => NotificationRequest::Status {},
        "pab_list_notifications" => NotificationRequest::List {
            package_id: text("package_id")?,
            limit: u16::try_from(number("limit", 16)?).map_err(|_| "invalid limit")?,
        },
        "pab_wait_notification" => NotificationRequest::Wait {
            package_id: text("package_id")?,
            limit: u16::try_from(number("limit", 16)?).map_err(|_| "invalid limit")?,
            after_revision: args
                .get("after_revision")
                .and_then(Value::as_u64)
                .ok_or("after_revision required")?,
            timeout_ms: u32::try_from(number("timeout_ms", 10000)?)
                .map_err(|_| "invalid timeout_ms")?,
        },
        "pab_open_notification" => NotificationRequest::Open {
            notification_ref: text("notification_ref")?,
        },
        "pab_dismiss_notification" => NotificationRequest::Dismiss {
            notification_ref: text("notification_ref")?,
        },
        _ => return Err("unknown notification tool".into()),
    };
    query.validate().map_err(str::to_owned)?;
    Ok((id, SystemQuery::Notifications { query }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_validation_and_request_conversion_are_consistent() {
        assert_eq!(tools().len(), 5);
        assert!(
            parse(
                "pab_wait_notification",
                &json!({"package_id":"com.pixels.fixture"})
            )
            .is_err()
        );
        let (_, q) = parse(
            "pab_wait_notification",
            &json!({"package_id":"com.pixels.fixture","after_revision":0}),
        )
        .unwrap();
        assert!(!q.is_mutation());
        assert!(
            parse(
                "pab_open_notification",
                &json!({"notification_ref":"not-a-ref"})
            )
            .is_err()
        );
        for tool in tools() {
            assert!(
                super::super::mcp_catalog::tools()
                    .iter()
                    .any(|t| t["name"] == tool["name"])
            );
        }
    }
}
