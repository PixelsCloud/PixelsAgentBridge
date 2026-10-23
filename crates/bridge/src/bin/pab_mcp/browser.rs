use pab_bridge::BridgeRuntime;
use pab_protocol::{
    AppQuery, BrowserAction, BrowserRequest, ExecutionSelection, RequestId, SystemQuery,
};
use serde_json::{Value, json};

pub(super) fn handles(name: &str) -> bool {
    BrowserAction::ALL
        .iter()
        .any(|action| format!("pab_{}", action.name()) == name)
}

fn string(max: usize) -> Value {
    json!({"type":"string","minLength":1,"maxLength":max})
}
fn fields(action: BrowserAction) -> (Value, Vec<&'static str>, &'static str) {
    use BrowserAction::*;
    match action {
        SetupInfo => (
            json!({}),
            vec![],
            "Check whether the target interactive user can launch the Pixels-owned browser. Pixels uses installed Chrome, or Edge if Chrome is absent, with its own persistent profile for this Pixels account and OS user. No extension or access to the user's ordinary browser profile is needed.",
        ),
        Attach => (
            json!({"mode":{"type":"string","enum":["pixels"],"default":"pixels"}}),
            vec![],
            "Start or reopen the Pixels-owned Chrome or Edge window. Omit mode. The browser uses a persistent profile for the signed-in Pixels account under the target OS user, so website logins backed by persistent browser storage can be reused on later tasks. Some sites require sign-in again after browser close or session expiry. Never attach to the user's ordinary Chrome/Edge profile. If a site requires login, call pab_browser_request_handoff, then observe the choice through pab_browser_get_session; do not ask the user to confirm again in chat.",
        ),
        GetSession => (
            json!({}),
            vec![],
            "Read the exact Pixels browser session state, including the user's choice in the Pixels sign-in dialog. While awaiting_user, wait and check again without asking the user to reply in chat. ready_to_resume means the user clicked the dialog's completion button: call pab_browser_resume, then verify the website is actually signed in. cancelled means the user confirmed cancellation: stop this browser task; do not resume it. timed_out or expired means the session ended and a new task is needed. The dialog choice, not a chat reply, is authoritative.",
        ),
        Close | Snapshot | Screenshot | PdfSave => (
            json!({}),
            vec![],
            "Operate on the exact browser session. The browser helper must still be connected and authorized.",
        ),
        Resume => (
            json!({}),
            vec![],
            "Resume when pab_browser_get_session returns ready_to_resume after the target user clicks the completion button in Pixels Desktop; no separate chat confirmation is needed. Then inspect the same site again to verify that sign-in actually succeeded; the dialog button alone is not proof. If the site still requires sign-in, pause again and tell the user. Never request a password, verification code, or QR-code content through the Agent.",
        ),
        Downloads => (
            json!({"filename":string(255)}),
            vec![],
            "List only downloads that the Pixels browser runtime reported for this session. An exact filename filters these tracked files. This tool does not scan the user's ordinary Downloads folder.",
        ),
        Open => (
            json!({"url":string(2048)}),
            vec!["url"],
            "Open a new authorized tab at an http(s) URL.",
        ),
        Navigate => (
            json!({"url":string(2048),"direction":{"type":"string","enum":["back","forward","reload"]}}),
            vec![],
            "Navigate the selected authorized tab. Supply exactly url or direction.",
        ),
        Tabs => (
            json!({"action":{"type":"string","enum":["list","new","select"]},"index":{"type":"integer","minimum":0,"maximum":1000},"url":string(2048)}),
            vec!["action"],
            "List, create, or select tabs in the Pixels-owned browser. Closing tabs is unavailable because this Playwright version does not expose stable tab identities.",
        ),
        Find => (
            json!({"text":string(512),"regex":string(512)}),
            vec![],
            "Search the current accessibility snapshot; supply text or regex.",
        ),
        Extract => (
            json!({"target":string(256),"kind":{"type":"string","enum":["text","link","attributes","table","value"]},"limit":{"type":"integer","minimum":1,"maximum":100}}),
            vec!["target", "kind"],
            "Extract bounded data from one referenced element.",
        ),
        Wait => (
            json!({"text":string(512),"text_gone":string(512),"time":{"type":"number","minimum":0,"maximum":30}}),
            vec![],
            "Wait up to 30 seconds for page text or a short duration.",
        ),
        Click => (
            json!({"target":string(256),"element":string(256),"double_click":{"type":"boolean"},"button":{"type":"string","enum":["left","right","middle"]}}),
            vec!["target"],
            "Click an exact element reference from the current snapshot.",
        ),
        Fill => (
            json!({"target":string(256),"element":string(256),"text":{"type":"string","maxLength":4096},"slowly":{"type":"boolean"},"submit":{"type":"boolean"}}),
            vec!["target", "text"],
            "Fill or clear a normal editable field. Never enter a website password, verification code, or other sign-in secret through this tool; request user handoff instead.",
        ),
        FillForm => (
            json!({"fields":{"type":"array","minItems":1,"maxItems":20,"items":{"type":"object","properties":{"name":string(256),"type":{"type":"string","enum":["textbox","checkbox","radio","combobox","slider"]},"target":string(256),"value":{"type":"string","maxLength":4096}},"required":["name","type","target","value"],"additionalProperties":false}}}),
            vec!["fields"],
            "Fill several normal fields in order; partial effects are never replayed automatically. A combobox field value is its visible option label (case-sensitive), not the HTML option value; pab_browser_select_option accepts option values. Checkbox/radio values are strings true or false. Use user handoff for passwords, verification codes, and other sign-in secrets.",
        ),
        PressKey => (
            json!({"key":string(128)}),
            vec!["key"],
            "Press one Playwright key or key chord in the authorized tab.",
        ),
        SelectOption => (
            json!({"target":string(256),"element":string(256),"values":{"type":"array","minItems":1,"maxItems":20,"items":string(256)}}),
            vec!["target", "values"],
            "Select native dropdown options.",
        ),
        SetChecked => (
            json!({"target":string(256),"checked":{"type":"boolean"}}),
            vec!["target", "checked"],
            "Set the desired checkbox or radio state; does not toggle blindly.",
        ),
        Hover => (
            json!({"target":string(256),"element":string(256)}),
            vec!["target"],
            "Hover over an exact element reference.",
        ),
        Scroll => (
            json!({"direction":{"type":"string","enum":["up","down"]},"pages":{"type":"integer","minimum":1,"maximum":10}}),
            vec!["direction"],
            "Scroll the authorized page by bounded page increments.",
        ),
        Drag => (
            json!({"start_target":string(256),"end_target":string(256)}),
            vec!["start_target", "end_target"],
            "Drag between two exact element references.",
        ),
        HandleDialog => (
            json!({"accept":{"type":"boolean"},"prompt_text":string(4096)}),
            vec!["accept"],
            "Accept or dismiss a pending browser dialog.",
        ),
        FileUpload => (
            json!({"paths":{"type":"array","minItems":1,"maxItems":10,"items":string(4096)}}),
            vec!["paths"],
            "Upload target-device files already present under the selected OS user's access.",
        ),
        RequestHandoff => (
            json!({"reason":string(512)}),
            vec!["reason"],
            "When a site requires sign-in or verification, pause Agent browser access and tell the user once to complete it in this Pixels browser on the target device and click the Pixels dialog button. The dialog reports the user's choice through pab_browser_get_session: poll it while awaiting_user; ready_to_resume means continue with pab_browser_resume, cancelled means stop. Do not ask the user to select or report the same choice again in chat. Do not collect credentials or claim remote desktop control is available. If the user cannot access that device, leave the task waiting or cancel it. After resuming, inspect the site's real post-login state before continuing.",
        ),
        Pointer => (
            json!({"action":{"type":"string","enum":["click","move","wheel"]},"x":{"type":"integer","minimum":0,"maximum":10000},"y":{"type":"integer","minimum":0,"maximum":10000},"delta_x":{"type":"integer","minimum":-1000,"maximum":1000},"delta_y":{"type":"integer","minimum":-1000,"maximum":1000},"button":{"type":"string","enum":["left","right","middle"]}}),
            vec!["action"],
            "Operate by CSS-pixel coordinates on the selected Pixels browser tab. Use a current browser screenshot before clicking; coordinates may become stale.",
        ),
        Drop => (
            json!({"target":string(256),"paths":{"type":"array","minItems":1,"maxItems":10,"items":string(4096)},"data":{"type":"object","propertyNames":{"enum":["text/plain","text/uri-list"]},"additionalProperties":{"type":"string","maxLength":4096}}}),
            vec!["target"],
            "Drop target-device files or explicit text data onto a referenced page element.",
        ),
        ConsoleMessages => (
            json!({"level":{"type":"string","enum":["error","warning","info","debug"]},"all":{"type":"boolean"}}),
            vec![],
            "Read bounded console messages from the selected Pixels browser tab for page diagnosis.",
        ),
        NetworkRequests => (
            json!({"static":{"type":"boolean"},"filter":string(256)}),
            vec![],
            "Read a bounded network-request summary for the selected Pixels browser tab; URLs may contain site data.",
        ),
    }
}

pub(super) fn tools() -> Vec<Value> {
    BrowserAction::ALL.into_iter().map(|action| {
        let (extras, mut required, description) = fields(action);
        let mut properties = json!({
            "device_code":{"type":"string","pattern":"^[0-9]{9}$"},
            "execution":{"type":"object","properties":{"mode":{"const":"desktop_user"},"context_ref":{"type":"string","format":"uuid"}},"required":["mode","context_ref"],"additionalProperties":false},
            "session_ref":{"type":"string","format":"uuid"},
            "request_id":{"type":"string","format":"uuid"}
        });
        properties.as_object_mut().unwrap().extend(extras.as_object().unwrap().clone());
        required.extend(["device_code","execution"]);
        if !matches!(action, BrowserAction::Attach | BrowserAction::SetupInfo) { required.push("session_ref"); }
        json!({"name":format!("pab_{}",action.name()),"description":format!("{description} Requires an upgraded Executor and interactive desktop_user selected by pab_list_execution_contexts. request_id is optional: omit it to generate a UUID automatically, or supply a UUID to retry the same operation safely. Preserve the returned operation_ref and query the original operation after an uncertain result; browser effects are not replayed."),"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":!action.is_mutation(),"destructiveHint":action.is_mutation(),"idempotentHint":!action.is_mutation(),"openWorldHint":true}})
    }).collect()
}

pub(super) fn parse(name: &str, args: &Value) -> Result<(RequestId, SystemQuery), String> {
    let action = BrowserAction::ALL
        .into_iter()
        .find(|a| format!("pab_{}", a.name()) == name)
        .ok_or("unknown browser tool")?;
    let mut values = args
        .as_object()
        .cloned()
        .ok_or("arguments must be an object")?;
    values.remove("device_code");
    let id = values
        .remove("request_id")
        .map(serde_json::from_value::<RequestId>)
        .transpose()
        .map_err(|_| "invalid request_id")?
        .unwrap_or_default();
    let execution: ExecutionSelection = serde_json::from_value(
        values
            .remove("execution")
            .ok_or("desktop_user execution required")?,
    )
    .map_err(|_| "invalid execution")?;
    let session_ref = values
        .remove("session_ref")
        .map(serde_json::from_value::<RequestId>)
        .transpose()
        .map_err(|_| "invalid session_ref")?;
    let request = BrowserRequest {
        action,
        session_ref,
        arguments: Value::Object(values),
        owner: None,
        pixels_user: None,
    };
    request.validate().map_err(str::to_owned)?;
    let query = SystemQuery::Applications {
        execution,
        query: AppQuery::Browser { request },
    };
    query.validate().map_err(str::to_owned)?;
    Ok((id, query))
}

pub(super) async fn call(
    runtime: &BridgeRuntime,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    let (id, query) = parse(name, args)?;
    let device = super::mcp_tools::resolve_target(runtime, args).await?;
    let target = runtime
        .current_environment(device)
        .await
        .map_err(|e| e.to_string())?;
    let reply = runtime
        .system_query(device, id, query)
        .await
        .map_err(|e| e.to_string())?;
    Ok(
        json!({"device_ref":device,"operation_ref":{"device_code":args["device_code"],"operation_id":id,"kind":reply.kind},"result":reply,"os_reminder":target.compact_reminder()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_tools_have_strict_schemas_and_bound_context() {
        let tools = tools();
        assert_eq!(tools.len(), 31);
        for tool in &tools {
            assert!(tool["inputSchema"]["additionalProperties"] == false);
        }
        let args = json!({"device_code":"123456789","request_id":RequestId::new(),"execution":{"mode":"desktop_user","context_ref":pab_protocol::ExecutionContextRef::new()}});
        let (_, query) = parse("pab_browser_attach", &args).unwrap();
        assert_eq!(query.required_version(), 14);
        let without_id = json!({"device_code":"123456789","execution":{"mode":"desktop_user","context_ref":pab_protocol::ExecutionContextRef::new()}});
        let (generated_id, _) = parse("pab_browser_attach", &without_id).unwrap();
        let (second_id, _) = parse("pab_browser_attach", &without_id).unwrap();
        assert_ne!(generated_id, second_id);
        let (provided_id, _) = parse("pab_browser_attach", &args).unwrap();
        assert_eq!(provided_id, serde_json::from_value(args["request_id"].clone()).unwrap());
        assert!(!tools.iter().find(|tool| tool["name"] == "pab_browser_attach").unwrap()["inputSchema"]["required"].as_array().unwrap().contains(&json!("request_id")));
        assert!(parse("pab_browser_click", &args).is_err());
        let setup = json!({"device_code":"123456789","execution":{"mode":"desktop_user","context_ref":pab_protocol::ExecutionContextRef::new()}});
        let (_, query) = parse("pab_browser_setup_info", &setup).unwrap();
        assert_eq!(query.required_version(), 14);
        let edge = json!({"device_code":"123456789","request_id":RequestId::new(),"mode":"existing_edge","profile_dir_name":"Default","execution":{"mode":"desktop_user","context_ref":pab_protocol::ExecutionContextRef::new()}});
        assert!(parse("pab_browser_attach", &edge).is_err());
    }
}
