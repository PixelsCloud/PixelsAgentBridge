use pab_bridge::BridgeRuntime;
use pab_protocol::{RequestId, SystemQuery};
use serde_json::{Value, json};

pub(super) fn tools() -> Vec<Value> {
    let base = json!({"device_code":{"type":"string","pattern":"^[0-9]{9}$"},"request_id":{"type":"string","format":"uuid"}});
    let filter = json!({"type":"string","minLength":1,"maxLength":256});
    let limit = json!({"type":"integer","minimum":1,"maximum":1000,"default":100});
    let cpu = json!({"type":"boolean","default":false});
    [
        ("pab_system_info","Query target OS/host/kernel/architecture/uptime, CPU, RAM/swap and Executor identity through sysinfo. sample_cpu=true default samples utilization twice at the library minimum interval. include_gpu=false default; true uses NVIDIA NVML, GPU unavailable/unsupported only affects GPU section. GPU backend only covers NVIDIA; unavailable is not evidence that no GPUs exist.",json!({"sample_cpu":{"type":"boolean","default":true},"include_gpu":{"type":"boolean","default":false}}),vec!["device_code"]),
        ("pab_list_disks","Query target disks/mounts with filesystem, capacity/free bytes, disk kind and removable/readonly flags through sysinfo. Refresh inventory per query; max 1000 entries, no paging.",json!({"limit":limit}),vec!["device_code"]),
        ("pab_list_processes","Collect one target process list, then apply optional pid, case-insensitive literal name substring and exact user name/ID filters. No live pagination or atomic OS snapshot. sample_cpu=false default; true takes two samples. Output includes PID, parent, name, executable, user, start time, state, RAM and optional CPU; no command arguments/environment. New processes without two samples have null CPU.",json!({"pid":{"type":"integer","minimum":1,"maximum":4294967295u64},"name":filter,"user":filter,"limit":limit,"sample_cpu":cpu}),vec!["device_code"]),
        ("pab_get_process","Query one current PID through sysinfo; a missing/unobservable process fails. PID/start time are observation only, not a safe termination token; no process termination is performed. sample_cpu=false default.",json!({"pid":{"type":"integer","minimum":1,"maximum":4294967295u64},"sample_cpu":cpu}),vec!["device_code","pid"]),
        ("pab_list_network_interfaces","Query target interface name, IP/prefix, MAC, MTU, operational state and cumulative received/transmitted bytes through sysinfo. Optional case-insensitive literal name substring filter. Not a TCP/UDP connection table. At most 32 IP addresses/interface, 1000 entries; no paging.",json!({"name":filter,"limit":limit}),vec!["device_code"]),
    ].into_iter().map(|(name,description,extra,required)| {
        let mut properties=base.clone();properties.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        json!({"name":name,"description":format!("{description} Requires system-query capability v1. Synchronous, bounded 32 KiB result, timestamps and explicit truncation; unavailable fields are null. CPU basis points: 10000 = 100%, per-process CPU may exceed 10000. Keep request_id to retrieve the SAME sampled result using pab_get_operation; omit it for a new sample. Queries cannot be cancelled. OS/driver calls may delay completion; no hard interruption of blocking native calls."),"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}})
    }).collect()
}
pub(super) fn parse(name: &str, args: &Value) -> Result<(RequestId, SystemQuery), String> {
    let id = args
        .get("request_id")
        .map(|v| {
            v.as_str()
                .ok_or("invalid request_id")?
                .parse()
                .map_err(|_| "invalid request_id")
        })
        .transpose()?
        .unwrap_or_default();
    let cpu = args
        .get("sample_cpu")
        .and_then(Value::as_bool)
        .unwrap_or(name == "pab_system_info");
    let limit = u16::try_from(args.get("limit").and_then(Value::as_u64).unwrap_or(100))
        .map_err(|_| "invalid limit")?;
    let filter = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_owned);
    let pid = args
        .get("pid")
        .map(|v| {
            v.as_u64()
                .ok_or("invalid pid")
                .and_then(|n| u32::try_from(n).map_err(|_| "invalid pid"))
        })
        .transpose()?;
    let q = match name {
        "pab_system_info" => SystemQuery::Info {
            include_gpu: args
                .get("include_gpu")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            sample_cpu: cpu,
        },
        "pab_list_disks" => SystemQuery::Disks { limit },
        "pab_list_processes" => SystemQuery::Processes {
            pid,
            name: filter("name"),
            user: filter("user"),
            limit,
            sample_cpu: cpu,
        },
        "pab_get_process" => SystemQuery::Process {
            pid: pid.ok_or("pid required")?,
            sample_cpu: cpu,
        },
        "pab_list_network_interfaces" => SystemQuery::Networks {
            name: filter("name"),
            limit,
        },
        _ => return Err("unknown system query tool".into()),
    };
    q.validate().map_err(str::to_owned)?;
    Ok((id, q))
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
    let r = runtime
        .system_query(device, id, query)
        .await
        .map_err(|e| e.to_string())?;
    Ok(
        json!({"device_ref":device,"operation_ref":{"device_code":args["device_code"],"operation_id":id,"kind":r.kind},"result":r,"os_reminder":target.compact_reminder()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_defaults_and_filters_are_strict_and_have_no_pagination() {
        let (_, q) = parse("pab_system_info", &json!({})).unwrap();
        assert!(matches!(
            q,
            SystemQuery::Info {
                include_gpu: false,
                sample_cpu: true
            }
        ));
        let (_, q) = parse("pab_list_processes", &json!({})).unwrap();
        assert!(matches!(
            q,
            SystemQuery::Processes {
                limit: 100,
                sample_cpu: false,
                ..
            }
        ));
        for args in [
            json!({"device_code":"123456789","offset":1}),
            json!({"device_code":"123456789","limit":1001}),
            json!({"device_code":"123456789","sample_cpu":"yes"}),
        ] {
            assert!(
                super::super::mcp_catalog::validate_arguments("pab_list_processes", &args).is_err()
            );
        }
        assert!(parse("pab_get_process", &json!({"pid":4294967296u64})).is_err());
        assert!(parse("pab_list_processes", &json!({"name":"中".repeat(86)})).is_err());
    }
}
