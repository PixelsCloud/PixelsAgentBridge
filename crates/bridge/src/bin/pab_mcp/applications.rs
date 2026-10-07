use pab_protocol::{
    AppActionRequest, AppListRequest, AppQuery, ExecutionSelection, RequestId, SystemQuery,
};
use serde_json::{Value, json};

pub(super) fn handles(name: &str) -> bool {
    matches!(name, "pab_list_apps" | "pab_launch_app" | "pab_open_file")
}

pub(super) fn tools() -> Vec<Value> {
    let target = json!({"type":"object","properties":{
        "kind":{"type":"string","enum":["id","path"]},
        "id":{"type":"string","minLength":1,"maxLength":4096},
        "path":{"type":"string","minLength":1,"maxLength":4096}
    },"required":["kind"],"additionalProperties":false,
        "description":"Exactly {kind:id,id:<OS identifier>} or {kind:path,path:<absolute native application path>}. Never include both id and path."});
    ["pab_list_apps", "pab_launch_app", "pab_open_file"].into_iter().map(|name| {
        let mutation = name != "pab_list_apps";
        let mut properties = json!({
            "device_code":{"type":"string","pattern":"^[0-9]{9}$"},
            "request_id":{"type":"string","format":"uuid"},
            "execution":{"type":"object","properties":{
                "mode":{"type":"string","enum":["desktop_user"]},
                "context_ref":{"type":"string","format":"uuid"}
            },"required":["mode","context_ref"],"additionalProperties":false}
        });
        let mut required = vec!["device_code", "execution"];
        let description = match name {
            "pab_list_apps" => {
                properties["scope"] = json!({"type":"string","enum":["installed","running"],"default":"installed"});
                properties["search"] = json!({"type":"string","maxLength":256,"default":""});
                properties["limit"] = json!({"type":"integer","minimum":1,"maximum":200,"default":100});
                "Discover installed applications or running instances for the selected desktop user. Windows uses AppsFolder and visible-window processes; macOS uses standard application directories and NSWorkspace. Case-insensitive literal search before limit; bounded 32 KiB snapshot, no live pagination. Narrow search if truncated. OS IDs/paths and process instances are distinct; installed entries need not be running. Inaccessible fields/instances are omitted with warnings."
            }
            "pab_launch_app" => {
                properties["application"] = target.clone(); required.push("application");
                properties["new_instance"] = json!({"type":"boolean","default":false});
                "Launch or activate an application by its exact OS ID from pab_list_apps or absolute native application path (Windows executable/macOS .app). Uses Windows Shell/macOS NSWorkspace without arbitrary command arguments. The OS may reuse an existing instance. On macOS, new_instance=true requests a fresh instance (system-query v13/helper v2); Windows/Linux reject it without launching. Use only when a separate instance is intended, for example a background helper occupies the same app bundle. An app may still apply its own single-instance policy; observe the returned process/window."
            }
            _ => {
                properties["path"] = json!({"type":"string","minLength":1,"maxLength":4096});
                properties["application"] = target.clone(); required.push("path");
                "Open an existing absolute local file on the target device with a specified application, or omit application to use the user's system default. Paths refer to the target, not the agent's machine. No URL schemes or arbitrary arguments."
            }
        };
        json!({"name":name,"description":format!("{description} Requires Windows/macOS system-query v12 and an application-capable user helper. Linux headless is unsupported. execution must be a desktop_user selection returned by pab_list_execution_contexts on this connection; never substitute service or user mode. Missing/locked/changed desktop fails without fallback. Preserve request_id and query the original operation_ref with pab_get_operation when running/unconfirmed; disconnect or timeout never justifies automatic replay. Actions cannot be cancelled or rolled back. request_accepted means OS acceptance, not a ready window or verified file content; use pab_list_windows and existing window/UI tools to observe, focus or request normal close. OS calls have no guaranteed hard interruption."),
            "inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
            "annotations":{"readOnlyHint":!mutation,"destructiveHint":mutation,"idempotentHint":!mutation,"openWorldHint":false}})
    }).collect()
}

pub(super) fn parse(name: &str, args: &Value) -> Result<(RequestId, SystemQuery), String> {
    let mut fields = args
        .as_object()
        .ok_or("arguments must be an object")?
        .clone();
    fields.remove("device_code");
    let id = fields
        .remove("request_id")
        .map(serde_json::from_value::<RequestId>)
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let execution: ExecutionSelection = serde_json::from_value(
        fields
            .remove("execution")
            .ok_or("execution required: select a desktop_user context")?,
    )
    .map_err(|e| e.to_string())?;
    let query = match name {
        "pab_list_apps" => {
            fields.entry("scope").or_insert(json!("installed"));
            fields.entry("limit").or_insert(json!(100));
            AppQuery::List {
                request: serde_json::from_value::<AppListRequest>(Value::Object(fields))
                    .map_err(|e| e.to_string())?,
            }
        }
        "pab_launch_app" | "pab_open_file" => {
            if fields.contains_key("operation") {
                return Err("operation is determined by the tool name".into());
            }
            fields.insert(
                "operation".into(),
                json!(if name == "pab_launch_app" {
                    "launch"
                } else {
                    "open_file"
                }),
            );
            AppQuery::Execute {
                request: serde_json::from_value::<AppActionRequest>(Value::Object(fields))
                    .map_err(|e| e.to_string())?,
            }
        }
        _ => return Err("unknown application tool".into()),
    };
    let query = SystemQuery::Applications { execution, query };
    query.validate().map_err(str::to_owned)?;
    Ok((id, query))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base() -> Value {
        json!({"device_code":"123456789","execution":{"mode":"desktop_user","context_ref":pab_protocol::ExecutionContextRef::new()}})
    }
    #[test]
    fn application_tools_parse_defaults_and_reject_ambiguous_or_unsafe_inputs() {
        let args = base();
        let (_, query) = parse("pab_list_apps", &args).unwrap();
        assert_eq!(query.required_version(), 12);
        assert!(!query.is_mutation());
        assert!(matches!(
            query,
            SystemQuery::Applications {
                query: AppQuery::List {
                    request: AppListRequest { limit: 100, .. }
                },
                ..
            }
        ));
        for (key, value) in [
            ("page", json!(2)),
            ("limit", json!(0)),
            ("search", json!("中".repeat(86))),
            ("execution", json!({"mode":"service"})),
            (
                "execution",
                json!({"mode":"user","context_ref":pab_protocol::ExecutionContextRef::new()}),
            ),
        ] {
            let mut bad = args.clone();
            bad[key] = value;
            assert!(super::super::mcp_catalog::validate_arguments("pab_list_apps", &bad).is_err());
        }
        let mut launch = base();
        launch["application"] = json!({"kind":"id","id":"com.apple.TextEdit"});
        super::super::mcp_catalog::validate_arguments("pab_launch_app", &launch).unwrap();
        assert_eq!(
            parse("pab_launch_app", &launch).unwrap().1.kind(),
            "app_launch"
        );
        assert_eq!(
            parse("pab_launch_app", &launch)
                .unwrap()
                .1
                .required_version(),
            12
        );
        launch["new_instance"] = json!(true);
        super::super::mcp_catalog::validate_arguments("pab_launch_app", &launch).unwrap();
        assert_eq!(
            parse("pab_launch_app", &launch)
                .unwrap()
                .1
                .required_version(),
            13
        );
        launch.as_object_mut().unwrap().remove("new_instance");
        for bad_target in [
            json!({"kind":"id","id":"x","path":"/tmp/x"}),
            json!({"kind":"path"}),
            json!({"kind":"id","id":"x\u{0}"}),
            json!({"kind":"id","id":"x","username":"other"}),
        ] {
            launch["application"] = bad_target;
            assert!(
                super::super::mcp_catalog::validate_arguments("pab_launch_app", &launch).is_err()
            );
        }
        let mut open = base();
        open["path"] = json!("/tmp/中文 文件.txt");
        super::super::mcp_catalog::validate_arguments("pab_open_file", &open).unwrap();
        assert_eq!(
            parse("pab_open_file", &open).unwrap().1.kind(),
            "app_open_file"
        );
        for (key, value) in [
            ("args", json!(["--secret"])),
            ("url", json!("https://example.com")),
            ("operation", json!("launch")),
        ] {
            let mut bad = open.clone();
            bad[key] = value;
            assert!(super::super::mcp_catalog::validate_arguments("pab_open_file", &bad).is_err());
        }
        open.as_object_mut().unwrap().remove("execution");
        assert!(parse("pab_open_file", &open).is_err());
    }
}
