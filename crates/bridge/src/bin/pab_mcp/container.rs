use pab_protocol::{ContainerAction, ContainerQuery, RequestId, SystemQuery};
use serde_json::{Value, json};
pub(super) fn tools() -> Vec<Value> {
    let selector = json!({"type":"string","minLength":1,"maxLength":256});
    [
 ("pab_list_containers","List a bounded current Docker container inventory. all=false defaults to running only; name is a case-insensitive literal substring, states are exact, labels use Docker key or key=value filters. No live pagination or atomic snapshot; scan up to 10000 containers, at most limit returned.",json!({"all":{"type":"boolean","default":false},"name":selector,"states":{"type":"array","maxItems":7,"items":{"type":"string","enum":["created","restarting","running","removing","paused","exited","dead"]}},"labels":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":256}},"limit":{"type":"integer","minimum":1,"maximum":1000,"default":100}}),vec![]),
 ("pab_get_container","Inspect one Docker container by exact name or full 64-character ID; abbreviated IDs are rejected. Returns state/health, image, ports, mounts, networks, restart policy and log driver. Excludes environment, command arguments and raw inspect payload; labels and log text may contain application-provided data.",json!({"container":selector}),vec!["container"]),
 ("pab_container_logs","Read finite Docker logs, follow=false. since/until are Unix epoch seconds up to 2147483647, until>0; tail uses Docker's native tail-line selection; no per-stream line count is guaranteed. Return stdout/stderr grouped separately with no cross-stream ordering guarantee; TTY output is merged console data and cannot select stderr alone. Non-UTF8 is displayed with explicit replacement warning. Result is a bounded tail, not a lossless resumable log cursor; unsupported logging drivers fail explicitly.",json!({"container":selector,"since":{"type":"integer","minimum":0,"maximum":2147483647},"until":{"type":"integer","minimum":1,"maximum":2147483647},"tail":{"type":"integer","minimum":1,"maximum":1000,"default":200},"stdout":{"type":"boolean","default":true},"stderr":{"type":"boolean","default":true},"timestamps":{"type":"boolean","default":true},"max_bytes":{"type":"integer","minimum":1024,"maximum":16384,"default":16384}}),vec!["container"]),
 ("pab_container_control","Asynchronously start, stop or restart an existing Docker container. Resolves and pins full ID and engine identity; never targets a replacement with the same name. start/stop already in the desired state sends no action. restart requires running state and changed StartedAt. Conflicting PAB controls return busy, including unresolved previously submitted controls until their original result is reconciled. stop/restart use Docker's normal stop timeout, after which Docker may kill the container. No creation/removal, image pulls, exec, pause/unpause or rollback.",json!({"container":selector,"control":{"type":"string","enum":["start","stop","restart"]},"stop_timeout_seconds":{"type":"integer","minimum":0,"maximum":120,"default":10}}),vec!["container","control"])
 ].into_iter().map(|(name,description,extra,required)|{let mutation=name=="pab_container_control";let mut properties=json!({"device_code":{"type":"string","pattern":"^[0-9]{9}$"},"request_id":{"type":"string","format":"uuid"},"timeout_ms":{"type":"integer","minimum":100,"maximum":300000,"default":30000}});properties.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());let mut fields=vec!["device_code"];fields.extend(required);json!({"name":name,"description":format!("{description} Requires system-query capability v6 and a local Docker Engine accessible to the Executor OS identity through Unix socket/Windows named pipe (local DOCKER_HOST or standard socket). Docker CLI contexts, remote TCP/SSH endpoints and the logged-in user's credentials are not implicitly reused. Uses Bollard API/version negotiation, no shell. Result up to 32 KiB with explicit truncation. Keep request_id; query pab_get_operation for ORIGINAL persisted result; the same ID never reruns an action. Control returns running after about 250ms. pab_cancel_operation stops waiting, cannot undo Docker-accepted actions. Timeout/cancel/restart may leave unconfirmed effects; later query can inspect the pinned container without resubmission."),"inputSchema":{"type":"object","properties":properties,"required":fields,"additionalProperties":false},"annotations":{"readOnlyHint":!mutation,"destructiveHint":mutation,"idempotentHint":!mutation,"openWorldHint":false}})}).collect()
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
    let timeout = fields.remove("timeout_ms").unwrap_or(json!(30000));
    let (action, defaults) = match name {
        "pab_list_containers" => (
            "list",
            json!({"all":false,"name":null,"states":[],"labels":[],"limit":100}),
        ),
        "pab_get_container" => ("get", json!({})),
        "pab_container_logs" => (
            "logs",
            json!({"since":null,"until":null,"tail":200,"stdout":true,"stderr":true,"timestamps":true,"max_bytes":16384}),
        ),
        "pab_container_control" => ("control", json!({"stop_timeout_seconds":10})),
        _ => return Err("unknown container tool".into()),
    };
    for (k, v) in defaults.as_object().unwrap() {
        fields.entry(k.clone()).or_insert(v.clone());
    }
    fields.insert("action".into(), json!(action));
    let action: ContainerAction =
        serde_json::from_value(Value::Object(fields)).map_err(|e| e.to_string())?;
    let query: ContainerQuery =
        serde_json::from_value(json!({"action":action,"timeout_ms":timeout}))
            .map_err(|e| e.to_string())?;
    query.validate().map_err(str::to_owned)?;
    Ok((id, SystemQuery::Container { query }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_docker_tools_preserve_identity_and_reject_unknown_or_unsafe_parameters() {
        assert_eq!(tools().len(), 4);
        let id = RequestId::new();
        let (got, q) = parse(
            "pab_container_control",
            &json!({"container":"api","control":"restart","request_id":id}),
        )
        .unwrap();
        assert_eq!(got, id);
        assert_eq!(q.required_version(), 6);
        assert!(q.is_mutation());
        for (name, args) in [
            (
                "pab_container_control",
                json!({"container":"api","control":"remove"}),
            ),
            (
                "pab_container_logs",
                json!({"container":"api","follow":true}),
            ),
            (
                "pab_container_logs",
                json!({"container":"api","stdout":false,"stderr":false}),
            ),
            (
                "pab_get_container",
                json!({"container":"api","endpoint":"tcp://elsewhere"}),
            ),
            ("pab_list_containers", json!({"states":["any"]})),
        ] {
            assert!(parse(name, &args).is_err());
        }
    }
}
