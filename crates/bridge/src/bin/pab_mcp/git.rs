use pab_protocol::{GitAction, GitQuery, RequestId, SystemQuery};
use serde_json::{Value, json};

pub(super) fn tools() -> Vec<Value> {
    let files = json!({"type":"array","maxItems":128,"items":{"type":"string","minLength":1,"maxLength":4096}});
    let reference = json!({"type":"string","minLength":1,"maxLength":256});
    let remote = json!({"type":"string","pattern":"^[A-Za-z0-9_][A-Za-z0-9_.-]*$","maxLength":128});
    [
        ("status","Read native Git porcelain status with branch/HEAD/upstream/ahead/behind, literal paths, renames and conflicts. Untracked directories are summarized. Not an atomic repository snapshot.",json!({"limit":{"type":"integer","minimum":1,"maximum":1000,"default":100}}),vec![]),
        ("diff","Read unstaged diff by default, or staged=true, base commit versus worktree, or base/head commits. References resolve to commit IDs. paths are literal relative selections, never globs. External diff/textconv disabled. Untracked files are not included. Binary changes are summarized. Truncated or non-UTF-8 displayed diffs are not exact applyable patches.",json!({"staged":{"type":"boolean","default":false},"base":reference,"head":reference,"paths":files,"context_lines":{"type":"integer","minimum":0,"maximum":20,"default":3},"max_bytes":{"type":"integer","minimum":1024,"maximum":16384,"default":16384}}),vec![]),
        ("log","Read topologically ordered commits anchored at start (HEAD by default). Returns start_commit; pass its exact ID and skip for subsequent pages so branch movement does not change the starting history. Unborn HEAD returns an empty history.",json!({"start":reference,"skip":{"type":"integer","minimum":0,"maximum":100000,"default":0},"limit":{"type":"integer","minimum":1,"maximum":1000,"default":20}}),vec![]),
        ("commit","Stage only the selected literal relative files then commit ONLY those current worktree paths, preserving unrelated staged changes. files and message required; select individual files or tracked deletions, not directories; never add all files. Uses target Git author configuration and hooks. Failed/cancelled commit may leave selected files staged. No automatic identity, hook/signing bypass, or rollback.",json!({"files":{"type":"array","minItems":1,"maxItems":128,"items":{"type":"string","minLength":1,"maxLength":4096}},"message":{"type":"string","minLength":1,"maxLength":8192}}),vec!["files","message"]),
        ("checkout","Switch to an existing local branch without guessing remotes, or detach=true at a resolved commit reference. Native Git preserves compatible local changes and refuses overwrites/conflicts. No force, implicit stash, branch creation or reset.",json!({"reference":reference,"detach":{"type":"boolean","default":false}}),vec!["reference"]),
        ("fetch","Fetch a configured remote name, optionally one branch, using target native Git SSH/credential configuration. No URL/password argument or interactive prompt. Does not switch worktree. Network operation is asynchronous.",json!({"remote":remote,"branch":reference}),vec!["remote"]),
        ("pull","Pull specified remote/branch into the current branch. Explicit strategy required: ff_only, merge, or rebase. Requires existing attached HEAD and clean index/worktree; no implicit stash. Conflicts and partial repository effects are preserved for inspection, never auto-aborted. Asynchronous network operation.",json!({"remote":remote,"branch":reference,"strategy":{"type":"string","enum":["ff_only","merge","rebase"]}}),vec!["remote","branch","strategy"]),
        ("push","Push one explicit local branch to the same branch on a configured remote. force=false default; force=true uses an explicit lease against a freshly observed remote reference, never unconditional --force. Target credentials/SSH are reused; no URL/password argument. Asynchronous network operation. Timeout/cancellation cannot undo accepted remote changes.",json!({"remote":remote,"branch":reference,"force":{"type":"boolean","default":false}}),vec!["remote","branch"]),
    ].into_iter().map(|(action,description,extra,required)|{
        let mutation=!["status","diff","log"].contains(&action);
        let mut properties=json!({"device_code":{"type":"string","pattern":"^[0-9]{9}$"},"request_id":{"type":"string","format":"uuid"},"repo":{"type":"string","minLength":1,"maxLength":4096},"timeout_ms":{"type":"integer","minimum":100,"maximum":300000,"default":if ["fetch","pull","push"].contains(&action){300000}else{30000}}});
        properties.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let mut fields=vec!["device_code","repo"];fields.extend(required);
        json!({"name":format!("pab_git_{action}"),"description":format!("{description} repo must be absolute on the TARGET OS. Git/SSH/credential configuration belongs to the Executor process OS identity, which may be a service account rather than the logged-in user. Requests are limited to 60 KiB. Requires native Git (switch >=2.23) and system-query capability v5. Results up to 32 KiB with explicit truncation. Keep request_id and poll pab_get_operation for the ORIGINAL result; same ID/params never re-executes, even after restart. Mutations return running after about 250ms. pab_cancel_operation requests stopping the owned Git process; accepted local/remote effects are not rolled back and may remain unconfirmed. Git descendants/native hooks may outlive cancellation. Operations on the same discovered git directory fail busy while another PAB Git operation runs."),"inputSchema":{"type":"object","properties":properties,"required":fields,"additionalProperties":false},"annotations":{"readOnlyHint":!mutation,"destructiveHint":mutation,"idempotentHint":!mutation,"openWorldHint":(["fetch","pull","push"].contains(&action))}})
    }).collect()
}
pub(super) fn parse(name: &str, args: &Value) -> Result<(RequestId, SystemQuery), String> {
    let action = name.strip_prefix("pab_git_").ok_or("unknown Git tool")?;
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
    let repo = fields.remove("repo").ok_or("repo required")?;
    let timeout = fields
        .remove("timeout_ms")
        .unwrap_or(json!(if ["fetch", "pull", "push"].contains(&action) {
            300000
        } else {
            30000
        }));
    let defaults = match action {
        "status" => json!({"limit":100}),
        "diff" => {
            json!({"staged":false,"base":null,"head":null,"paths":[],"context_lines":3,"max_bytes":16384})
        }
        "log" => json!({"start":null,"skip":0,"limit":20}),
        "checkout" => json!({"detach":false}),
        "fetch" => json!({"branch":null}),
        "push" => json!({"force":false}),
        "commit" | "pull" => json!({}),
        _ => return Err("unknown Git tool".into()),
    };
    for (key, value) in defaults.as_object().unwrap() {
        fields.entry(key.clone()).or_insert(value.clone());
    }
    fields.insert("action".into(), json!(action));
    let action: GitAction =
        serde_json::from_value(Value::Object(fields)).map_err(|e| e.to_string())?;
    let query: GitQuery =
        serde_json::from_value(json!({"repo":repo,"action":action,"timeout_ms":timeout}))
            .map_err(|e| e.to_string())?;
    query.validate().map_err(str::to_owned)?;
    Ok((id, SystemQuery::Git { query }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eight_tools_have_strict_parameters_and_preserve_request_identity() {
        assert_eq!(tools().len(), 8);
        for tool in tools() {
            let name = tool["name"].as_str().unwrap();
            assert!(
                tool["description"]
                    .as_str()
                    .unwrap()
                    .contains("capability v5")
            );
            assert!(
                super::super::mcp_catalog::validate_arguments(
                    name,
                    &json!({"device_code":"123456789","repo":"C:\\repo","program":"cmd.exe"})
                )
                .is_err()
            );
        }
        let id = RequestId::new();
        let (returned, q) = parse(
            "pab_git_status",
            &json!({"repo":"C:\\repo","request_id":id}),
        )
        .unwrap();
        assert_eq!(returned, id);
        assert_eq!(q.required_version(), 5);
        assert!(!q.is_mutation());
        for (name, extra) in [
            ("status", json!({"limit":"100"})),
            ("commit", json!({"files":["../outside"],"message":"test"})),
            ("checkout", json!({"reference":"--force"})),
            ("diff", json!({"staged":true,"base":"HEAD"})),
            ("pull", json!({"remote":"origin","branch":"main"})),
            (
                "push",
                json!({"remote":"origin","branch":"main","force":"yes"}),
            ),
        ] {
            let mut args = json!({"repo":"C:\\repo"});
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(parse(&format!("pab_git_{name}"), &args).is_err(), "{name}");
        }
    }
}
