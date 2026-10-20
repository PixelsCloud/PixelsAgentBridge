use pab_protocol::{
    AppActionRequest, AppListRequest, AppPackageRequest, AppQuery, ExecutionSelection, RequestId,
    SystemQuery,
};
use serde_json::{Value, json};

pub(super) fn handles(name: &str) -> bool {
    package_operation(name).is_some()
        || matches!(name, "pab_list_apps" | "pab_launch_app" | "pab_open_file")
}

fn package_operation(name: &str) -> Option<&'static str> {
    Some(match name {
        "pab_inspect_app_package" => "inspect",
        "pab_install_app" => "install",
        "pab_uninstall_app" => "uninstall",
        "pab_get_app_change" => "status",
        "pab_resume_app_change" => "resume",
        _ => return None,
    })
}

fn package_tools() -> Vec<Value> {
    ["pab_inspect_app_package", "pab_install_app", "pab_uninstall_app", "pab_get_app_change", "pab_resume_app_change"].into_iter().map(|name| {
        let operation = package_operation(name).unwrap();
        let mutation = matches!(operation, "install" | "uninstall" | "resume");
        let mut properties = json!({
            "device_code":{"type":"string","pattern":"^[0-9]{9}$"},
            "request_id":{"type":"string","format":"uuid"},
            "execution":{"type":"object","properties":{
                "mode":{"type":"string","enum":["desktop_user"]},
                "context_ref":{"type":"string","format":"uuid"}
            },"required":["mode","context_ref"],"additionalProperties":false}
        });
        let mut required = vec!["device_code", "execution"];
        let description = match operation {
            "inspect" | "install" => {
                properties["path"] = json!({"type":"string","minLength":1,"maxLength":4096});
                required.push("path");
                if operation == "install" {
                    properties["package_id"] = json!({"type":"string","minLength":3,"maxLength":255});
                    properties["sha256"] = json!({"type":"string","pattern":"^[0-9a-f]{64}$"});
                    required.extend(["package_id", "sha256"]);
                    "Install a single base APK from an authorized virtual path. First inspect it, then pin its exact package_id and sha256. Updates preserve data; same/older versions, incompatible signers, split APKs and Pixels itself are rejected. Android requires the local install-source permission and system confirmation; never assume silent install. A returned change_ref records dispatch, not success."
                } else {
                    "Inspect a single base APK from an authorized virtual path, returning SHA-256, package ID, version and signing-certificate digests. Maximum 32 MiB. Does not install or grant permissions. Metadata is not a trust endorsement; Android performs final APK signature verification."
                }
            }
            "uninstall" => {
                properties["package_id"] = json!({"type":"string","minLength":3,"maxLength":255});
                properties["version_code"] = json!({"type":"integer","minimum":1,"maximum":i64::MAX});
                required.extend(["package_id", "version_code"]);
                "Request removal of an exact visible installed package/version through Android's system confirmation. This removes that application's data. Pixels itself and system applications are protected. A returned change_ref records dispatch, not completed removal."
            }
            "resume" => {
                properties["change_ref"] = json!({"type":"string","format":"uuid"}); required.push("change_ref");
                "Reopen the same pending Android system confirmation after pause, lock, background display deferral or reconnect. Does not create another installation/uninstallation. Only waiting_user_action changes can resume; observe the normal system UI."
            }
            _ => {
                properties["change_ref"] = json!({"type":"string","format":"uuid"}); required.push("change_ref");
                "Read a caller-scoped durable Android package-change fact. States: submitted, waiting_user_action, succeeded, cancelled, failed, unconfirmed. This never dispatches UI or repeats a package change. Original pab_get_operation results are dispatch snapshots; use this tool for the later OS result."
            }
        };
        json!({"name":name,"description":format!("{description} Requires Android and system-query v16, an unlocked interactive desktop_user selection from pab_list_execution_contexts, and enabled remote control. Preserve request_id; never replay after timeout/disconnect. Use change_ref with pab_get_app_change for asynchronous results. Confirm using normal system UI or existing mobile tools; Pixels cannot bypass OEM account/security requirements. Desktop package management is currently unsupported."),
            "inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
            "annotations":{"readOnlyHint":!mutation,"destructiveHint":mutation,"idempotentHint":!mutation,"openWorldHint":false}})
    }).collect()
}

pub(super) fn tools() -> Vec<Value> {
    let target = json!({"type":"object","properties":{
        "kind":{"type":"string","enum":["id","path"]},
        "id":{"type":"string","minLength":1,"maxLength":4096},
        "path":{"type":"string","minLength":1,"maxLength":4096}
    },"required":["kind"],"additionalProperties":false,
        "description":"Exactly {kind:id,id:<OS identifier>} or {kind:path,path:<absolute native application path>}. Never include both id and path."});
    let mut tools: Vec<Value> = ["pab_list_apps", "pab_launch_app", "pab_open_file"].into_iter().map(|name| {
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
        json!({"name":name,"description":format!("{description} Requires system-query v12. Windows/macOS need an application-capable user helper; Android uses the current unlocked app UID activity context. Linux headless is unsupported. Android installed lists contain only PackageManager-visible packages (details include version/enabled/system/launchable); running lists contain only Pixels. Android targets use package IDs, reject new_instance, and open authorized virtual-mount files as read-only snapshots up to 32 MiB. Observe Android activity results with pab_mobile_control. execution must be a desktop_user selection returned by pab_list_execution_contexts on this connection; never substitute service or user mode. Missing/locked/changed desktop fails without fallback. Preserve request_id and query the original operation_ref with pab_get_operation when running/unconfirmed; disconnect or timeout never justifies automatic replay. Actions cannot be cancelled or rolled back. request_accepted means OS acceptance, not a ready window or verified file content; observe with pab_mobile_control on Android, or pab_list_windows and existing window/UI tools on desktop. OS calls have no guaranteed hard interruption."),
            "inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
            "annotations":{"readOnlyHint":!mutation,"destructiveHint":mutation,"idempotentHint":!mutation,"openWorldHint":false}})
    }).collect();
    tools.extend(package_tools());
    tools
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
    let query = if let Some(operation) = package_operation(name) {
        if fields.contains_key("operation") {
            return Err("operation is determined by the tool name".into());
        }
        fields.insert("operation".into(), json!(operation));
        AppQuery::Package {
            request: serde_json::from_value::<AppPackageRequest>(Value::Object(fields))
                .map_err(|e| e.to_string())?,
        }
    } else {
        match name {
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
        }
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
    fn package_changes_validate_pins_and_preserve_original_request_identity() {
        let mut args = base();
        let id = RequestId::new();
        args["request_id"] = json!(id);
        args["path"] = json!("/workspace/fixture.apk");
        args["package_id"] = json!("com.pixels.fixture");
        args["sha256"] = json!("a".repeat(64));
        super::super::mcp_catalog::validate_arguments("pab_install_app", &args).unwrap();
        let (parsed, query) = parse("pab_install_app", &args).unwrap();
        assert_eq!(parsed, id);
        assert_eq!(query.kind(), "app_install");
        assert_eq!(query.required_version(), 16);
        assert!(query.is_mutation());
        for (field, value) in [
            ("sha256", json!("A".repeat(64))),
            ("package_id", json!("com..test")),
            ("silent", json!(true)),
        ] {
            let mut bad = args.clone();
            bad[field] = value;
            assert!(
                super::super::mcp_catalog::validate_arguments("pab_install_app", &bad).is_err()
            );
        }
        let mut status = base();
        status["change_ref"] = json!(id);
        assert!(
            !parse("pab_get_app_change", &status)
                .unwrap()
                .1
                .is_mutation()
        );
        assert!(
            parse("pab_resume_app_change", &status)
                .unwrap()
                .1
                .is_mutation()
        );
        status["operation"] = json!("install");
        assert!(parse("pab_get_app_change", &status).is_err());
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
