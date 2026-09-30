use pab_bridge::BridgeRuntime;
use pab_protocol::{ConnectionFilter, DnsRecordType, RequestId, ServiceControlAction, SystemQuery};
use serde_json::{Value, json};

pub(super) fn tools() -> Vec<Value> {
    let base = json!({"device_code":{"type":"string","pattern":"^[0-9]{9}$"},"request_id":{"type":"string","format":"uuid"}});
    let filter = json!({"type":"string","minLength":1,"maxLength":256});
    let limit = json!({"type":"integer","minimum":1,"maximum":1000,"default":100});
    let cpu = json!({"type":"boolean","default":false});
    [
        ("pab_terminate_process","Terminate exactly one target process using pid and termination_identity from pab_get_process. Default force=false, timeout_ms=5000. Windows sends WM_CLOSE only to top-level windows; windowless/console programs have no generic graceful exit and fail unless force=true (then immediate TerminateProcess). Linux sends SIGTERM through a retained pidfd identity lease (10 minutes, up to 256); optional force=true escalates to SIGKILL after timeout. Force exit confirmation has an extra 5s budget. Protects Executor/init. No process-tree termination.",json!({"pid":{"type":"integer","minimum":1,"maximum":4294967295u64},"identity":{"type":"string","minLength":1,"maxLength":256},"timeout_ms":{"type":"integer","minimum":100,"maximum":60000,"default":5000},"force":{"type":"boolean","default":false}}),vec!["device_code","pid","identity"]),
        ("pab_list_services","List Windows SCM Win32 services (not kernel drivers), or Linux systemd system .service units, including installed not_loaded units. Optional name is a case-insensitive literal substring; state exact and backend-specific. List rows are inventory summaries; use pab_get_service for configuration/PID/details. Filters before limit. No shell, no live pagination.",json!({"name":filter,"state":filter,"limit":limit}),vec!["device_code"]),
        ("pab_get_service","Get one service by exact native name, Linux requires .service suffix. Includes backend/state/substate/start mode/PID/executable/account/exit code and field errors. Linux uses system bus (not per-user systemd). Permission denied/unavailable fields are explicit; no configuration changes.",json!({"name":filter}),vec!["device_code","name"]),
        ("pab_service_control","Control a service: start/stop/restart or enable/disable startup configuration. Enable/disable never starts/stops it; Windows enable uses automatic startup, Linux enable uses persistent unit links without force. Runtime actions wait for actual state/job completion. No force-kill and no explicit dependent-service stop; systemd may execute the unit dependency transaction. Conflicting same-resource controls fail busy. Timeout defaults 30000 ms; an accepted OS action may continue after timeout and is never rolled back. Result preserves phase, changed, job path, observed state and error. Linux completed oneshot units that are inactive are reported as not active.",json!({"name":filter,"control":{"type":"string","enum":["start","stop","restart","enable","disable"]},"timeout_ms":{"type":"integer","minimum":100,"maximum":60000,"default":30000}}),vec!["device_code","name","control"]),
        ("pab_list_network_connections","Collect target TCP/UDP sockets through netstat2. Filters use exact IP/port/PID and canonical TCP state. Unknown owners have empty pids. UDP has no observable peer/state; TCP listeners have null peer. No atomic snapshot/live paging; max 1000 returned entries, 100000 scanned entries and soft 5s scan budget.",json!({"protocol":{"type":"string","enum":["tcp","udp"]},"family":{"type":"string","enum":["ipv4","ipv6"]},"local_address":{"type":"string","minLength":2,"maxLength":45},"remote_address":{"type":"string","minLength":2,"maxLength":45},"local_port":{"type":"integer","minimum":0,"maximum":65535},"remote_port":{"type":"integer","minimum":0,"maximum":65535},"pid":{"type":"integer","minimum":1,"maximum":4294967295u64},"state":{"type":"string","enum":["closed","listen","syn_sent","syn_received","established","fin_wait_1","fin_wait_2","close_wait","closing","last_ack","time_wait","delete_tcb","unknown"]},"limit":limit}),vec!["device_code"]),
        ("pab_resolve_dns","Resolve DNS on the target with Hickory and target system DNS configuration, cache disabled. Default record_type A; supports AAAA/CNAME/MX/NS/PTR/SOA/SRV/TXT. PTR accepts IP or reverse domain; other names must be ASCII/IDNA. Does not consult hosts, native OS resolver, mDNS or NRPT/VPN split-DNS policies. Timeout defaults 5000 ms, maximum 10000. Returns record names/types/TTL/text, truncation and source description, not a claim of native OS resolution equivalence.",json!({"name":{"type":"string","minLength":1,"maxLength":253},"record_type":{"type":"string","enum":["A","AAAA","CNAME","MX","NS","PTR","SOA","SRV","TXT"],"default":"A"},"timeout_ms":{"type":"integer","minimum":100,"maximum":10000,"default":5000},"limit":{"type":"integer","minimum":1,"maximum":100,"default":20}}),vec!["device_code","name"]),
        ("pab_list_sessions","List OS login sessions using Windows WTS or Linux logind (5s async collection deadline). This is not user accounts, MCP sessions or PAB terminals. User/state filters are exact; returned fields include ID/name/UID/state/activity/remote/seat/terminal/client/type and per-field errors. WTS may include service/listener sessions without a user. Unsupported backend/unavailable service fails explicitly.",json!({"user":filter,"state":filter,"limit":limit}),vec!["device_code"]),
        ("pab_system_info","Query target OS/host/kernel/architecture/uptime, CPU, RAM/swap and Executor identity through sysinfo. sample_cpu=true default samples utilization twice at the library minimum interval. include_gpu=false default; true uses NVIDIA NVML, GPU unavailable/unsupported only affects GPU section. GPU backend only covers NVIDIA; unavailable is not evidence that no GPUs exist.",json!({"sample_cpu":{"type":"boolean","default":true},"include_gpu":{"type":"boolean","default":false}}),vec!["device_code"]),
        ("pab_list_disks","Query target disks/mounts with filesystem, capacity/free bytes, disk kind and removable/readonly flags through sysinfo. Refresh inventory per query; max 1000 entries, no paging.",json!({"limit":limit}),vec!["device_code"]),
        ("pab_list_processes","Collect one target process list, then apply optional pid, case-insensitive literal name substring and exact user name/ID filters. No live pagination or atomic OS snapshot. sample_cpu=false default; true takes two samples. Output includes PID, parent, name, executable, user, start time, state, RAM and optional CPU; no command arguments/environment. New processes without two samples have null CPU.",json!({"pid":{"type":"integer","minimum":1,"maximum":4294967295u64},"name":filter,"user":filter,"limit":limit,"sample_cpu":cpu}),vec!["device_code"]),
        ("pab_get_process","Query one current PID through sysinfo; a missing/unobservable process fails. Returns termination_identity for pab_terminate_process on supported backends; null if identity is unavailable. Seconds-resolution start time must never substitute for this identity; process collection itself never terminates it. sample_cpu=false default.",json!({"pid":{"type":"integer","minimum":1,"maximum":4294967295u64},"sample_cpu":cpu}),vec!["device_code","pid"]),
        ("pab_list_network_interfaces","Query target interface name, IP/prefix, MAC, MTU, operational state and cumulative received/transmitted bytes through sysinfo. Optional case-insensitive literal name substring filter. Not a TCP/UDP connection table. At most 32 IP addresses/interface, 1000 entries; no paging.",json!({"name":filter,"limit":limit}),vec!["device_code"]),
    ].into_iter().map(|(name,description,extra,required)| {
        let mut properties=base.clone();properties.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let mutation=["pab_terminate_process","pab_service_control"].contains(&name);
        let version=if ["pab_terminate_process","pab_list_services","pab_get_service","pab_service_control"].contains(&name){3}else if ["pab_list_network_connections","pab_resolve_dns","pab_list_sessions"].contains(&name){2}else{1};
        let behavior=if mutation {"Asynchronous lifecycle control: returns running after about 250 ms if still active. Keep request_id and poll pab_get_operation for the original operation; disconnect/caller cancellation does not cancel an accepted action. Same request ID is never replayed, even after restart; a different request ID is a new action. Cannot cancel/undo lifecycle actions."} else {"Read-only query. Keep request_id to retrieve the SAME sampled result using pab_get_operation; omit it for a new sample. Queries cannot be cancelled. OS/driver calls may delay completion; no hard interruption of blocking native calls. CPU basis points: 10000 = 100%, per-process CPU may exceed 10000."};
        json!({"name":name,"description":format!("{description} Requires system-query capability v{version}. Bounded 32 KiB result with timestamps and explicit truncation; unavailable fields are null. {behavior}"),"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":!mutation,"destructiveHint":mutation,"idempotentHint":!mutation,"openWorldHint":name=="pab_resolve_dns"}})
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
    let limit = u16::try_from(
        args.get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(if name == "pab_resolve_dns" { 20 } else { 100 }),
    )
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
    let timeout = |default: u64| -> Result<u32, String> {
        u32::try_from(
            args.get("timeout_ms")
                .map(|v| v.as_u64().ok_or("invalid timeout_ms"))
                .transpose()?
                .unwrap_or(default),
        )
        .map_err(|_| "invalid timeout_ms".into())
    };
    let q = match name {
        "pab_terminate_process" => SystemQuery::TerminateProcess {
            pid: pid.ok_or("pid required")?,
            identity: filter("identity").ok_or("identity required")?,
            timeout_ms: timeout(5000)?,
            force: args
                .get("force")
                .map(|v| v.as_bool().ok_or("invalid force"))
                .transpose()?
                .unwrap_or(false),
        },
        "pab_list_services" => SystemQuery::Services {
            name: filter("name"),
            state: filter("state"),
            limit,
        },
        "pab_get_service" => SystemQuery::Service {
            name: filter("name").ok_or("name required")?,
        },
        "pab_service_control" => SystemQuery::ServiceControl {
            name: filter("name").ok_or("name required")?,
            control: serde_json::from_value::<ServiceControlAction>(
                args.get("control").cloned().ok_or("control required")?,
            )
            .map_err(|_| "invalid service control")?,
            timeout_ms: timeout(30000)?,
        },
        "pab_list_network_connections" => {
            let mut fields = serde_json::Map::new();
            for key in [
                "protocol",
                "family",
                "local_address",
                "remote_address",
                "local_port",
                "remote_port",
                "state",
                "pid",
            ] {
                fields.insert(key.into(), args.get(key).cloned().unwrap_or(Value::Null));
            }
            let filter: ConnectionFilter = serde_json::from_value(Value::Object(fields))
                .map_err(|e| format!("invalid connection filter: {e}"))?;
            SystemQuery::Connections { filter, limit }
        }
        "pab_resolve_dns" => SystemQuery::Dns {
            name: args
                .get("name")
                .and_then(Value::as_str)
                .ok_or("name required")?
                .into(),
            record_type: args
                .get("record_type")
                .cloned()
                .map(serde_json::from_value::<DnsRecordType>)
                .transpose()
                .map_err(|_| "invalid DNS record type")?
                .unwrap_or(DnsRecordType::A),
            timeout_ms: u32::try_from(
                args.get("timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(5000),
            )
            .map_err(|_| "invalid timeout_ms")?,
            limit,
        },
        "pab_list_sessions" => SystemQuery::Sessions {
            user: filter("user"),
            state: filter("state"),
            limit,
        },
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

#[cfg(test)]
mod c2_tests {
    use super::*;
    #[test]
    fn new_read_tools_validate_limits_addresses_and_defaults_before_dispatch() {
        let (_, q) = parse("pab_resolve_dns", &json!({"name":"example.com"})).unwrap();
        assert!(matches!(
            q,
            SystemQuery::Dns {
                record_type: DnsRecordType::A,
                timeout_ms: 5000,
                limit: 20,
                ..
            }
        ));
        let base = json!({"device_code":"123456789"});
        for (name, extra) in [
            (
                "pab_resolve_dns",
                json!({"name":"example.com","record_type":"ANY"}),
            ),
            (
                "pab_resolve_dns",
                json!({"name":"example.com","timeout_ms":10001}),
            ),
            ("pab_list_network_connections", json!({"local_port":65536})),
            ("pab_list_sessions", json!({"offset":1})),
        ] {
            let mut args = base.clone();
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(super::super::mcp_catalog::validate_arguments(name, &args).is_err());
        }
        assert!(
            parse(
                "pab_list_network_connections",
                &json!({"local_address":"bad"})
            )
            .is_err()
        );
        assert!(
            parse(
                "pab_list_network_connections",
                &json!({"protocol":"udp","state":"listen"})
            )
            .is_err()
        );
        for name in [
            "pab_list_network_connections",
            "pab_resolve_dns",
            "pab_list_sessions",
        ] {
            let t = tools().into_iter().find(|t| t["name"] == name).unwrap();
            assert_eq!(t["annotations"]["readOnlyHint"], true);
            assert!(t["description"].as_str().unwrap().contains("capability v2"));
        }
    }
}

#[cfg(test)]
mod c3_tests {
    use super::*;
    #[test]
    fn lifecycle_tools_are_typed_and_only_mutations_are_destructive() {
        let (_, q) = parse(
            "pab_terminate_process",
            &json!({"pid":123,"identity":"token"}),
        )
        .unwrap();
        assert!(matches!(
            q,
            SystemQuery::TerminateProcess {
                timeout_ms: 5000,
                force: false,
                ..
            }
        ));
        let (_, q) = parse(
            "pab_service_control",
            &json!({"name":"fixture","control":"restart"}),
        )
        .unwrap();
        assert!(matches!(
            q,
            SystemQuery::ServiceControl {
                control: ServiceControlAction::Restart,
                timeout_ms: 30000,
                ..
            }
        ));
        for args in [
            json!({"pid":1}),
            json!({"pid":1,"identity":"x","force":"yes"}),
            json!({"pid":1,"identity":"x","timeout_ms":60001}),
        ] {
            assert!(parse("pab_terminate_process", &args).is_err());
        }
        assert!(
            parse(
                "pab_service_control",
                &json!({"name":"fixture","control":"kill"})
            )
            .is_err()
        );
        for name in [
            "pab_terminate_process",
            "pab_service_control",
            "pab_list_services",
            "pab_get_service",
        ] {
            let t = tools().into_iter().find(|t| t["name"] == name).unwrap();
            let mutation = ["pab_terminate_process", "pab_service_control"].contains(&name);
            assert_eq!(t["annotations"]["readOnlyHint"], !mutation);
            assert_eq!(t["annotations"]["destructiveHint"], mutation);
        }
    }
}
