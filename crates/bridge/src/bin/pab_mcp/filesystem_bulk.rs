use pab_protocol::{FileOperationLimits, FileSystemAction};
use serde_json::{Value, json};

pub(super) fn tools() -> Vec<Value> {
    let common = json!({ "device_code": { "type":"string", "pattern":"^[0-9]{9}$" }, "path": { "type":"string", "minLength":1, "maxLength":4096 }, "request_id": { "type":"string", "format":"uuid" }, "max_entries": { "type":"integer", "minimum":1, "maximum":4096, "default":4096 }, "max_bytes": { "type":"integer", "minimum":0, "maximum":8589934592u64, "default":1073741824u64 }, "max_depth": { "type":"integer", "minimum":1, "maximum":64, "default":64 } });
    let destination = json!({ "type":"string", "minLength":1, "maxLength":4096 });
    let overwrite = json!({"type":"boolean","default":false});
    let recursive = json!({"type":"boolean","default":false});
    [
        ("pab_file_copy", "Copy within the target device. path is the exact source, destination is the exact target (no implicit basename append). Nonempty directories require recursive=true; overwrite=false by default, true permits merging directories and replacing regular files, preserving unrelated destination entries. Stream/stage/hash-check each file before publishing. No source deletion. Partial results possible.", json!({"destination":destination,"recursive":recursive,"overwrite":overwrite}), vec!["device_code","path","destination"]),
        ("pab_file_move", "Move within the target device, including across filesystems. Always copy/stage/verify destination before deleting source, including same-volume moves. Exact source/target paths; recursive=false, overwrite=false. Entire source manifest and destination files are rechecked before removal. Nonatomic tree operation: cancel/failure may leave published targets and remaining source entries; query source_removed and per-item results. Roots and overlapping paths rejected.", json!({"destination":destination,"recursive":recursive,"overwrite":overwrite}), vec!["device_code","path","destination"]),
        ("pab_file_delete", "Delete exactly path. Nonempty directories require recursive=true. Preflight bounded manifest; ordinary files and directories only, no links. Remove only planned entries, never remove_dir_all; new/unexpected entries stop removal. Roots/volume roots rejected. Report deleted entries and partial failure; no rollback.", json!({"recursive":recursive}), vec!["device_code","path"]),
        ("pab_archive_create", "Create a ZIP at path from 1..32 absolute sources (16 KiB total source path bytes). Each source basename is included; directories are recursively included within limits, including empty directories. Reject overlapping sources/output, duplicate or nonportable entry paths. Only ZIP, deflate level 6; staged ZIP is verified before publication. overwrite=false by default. Does not preserve ACL/ownership/extended metadata.", json!({"sources":{"type":"array","minItems":1,"maxItems":32,"items":destination},"overwrite":overwrite}), vec!["device_code","path","sources"]),
        ("pab_archive_extract", "Extract ZIP path into destination directory. Validate all entry paths/types/conflicts/quotas before writing: reject traversal, absolute/drive/reserved names, links, duplicate/case collisions, encryption, unsupported codecs and extreme compression ratio. Stored/deflate only; strict UTF-8 names. max_ratio=200 (1..1000). Max central directory 2 MiB. CRC, size and staged hash checked before each file publication; earlier files may remain if later CRC fails. overwrite=false controls file replacement; existing directories permitted. No ACL/mode restoration.", json!({"destination":destination,"overwrite":overwrite,"max_ratio":{"type":"integer","minimum":1,"maximum":1000,"default":200}}), vec!["device_code","path","destination"]),
    ].into_iter().map(|(name, description, extra, required)| {
        let mut properties = common.clone(); properties.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        json!({"name":name,"description":format!("{description} Requires filesystem v3. All actions run asynchronously after remote acceptance; preserve request_id, use pab_get_operation/pab_cancel_operation. Cancellation is cooperative; only cancelled confirms stopped. No replay of uncertain mutations. Max 4 bulk jobs/Executor, 30-minute cooperative deadline, default 4096 entries/1 GiB/64 depth (max 8 GiB). No intentional symlink/reparse traversal; external-writer races are not eliminated. Bounded item report may truncate, exact counts remain."),"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":false}})
    }).collect()
}

pub(super) fn parse(name: &str, args: &Value) -> Result<FileSystemAction, String> {
    let limits = FileOperationLimits {
        max_entries: u32::try_from(
            args.get("max_entries")
                .and_then(Value::as_u64)
                .unwrap_or(4096),
        )
        .map_err(|_| "invalid max_entries")?,
        max_bytes: args
            .get("max_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(1024 * 1024 * 1024),
        max_depth: u32::try_from(args.get("max_depth").and_then(Value::as_u64).unwrap_or(64))
            .map_err(|_| "invalid max_depth")?,
    };
    limits.validate().map_err(str::to_owned)?;
    let destination = || {
        args.get("destination")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "destination must be text".to_owned())
    };
    let recursive = args
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let overwrite = args
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(match name {
        "pab_file_copy" => FileSystemAction::Copy {
            destination: destination()?,
            recursive,
            overwrite,
            limits,
        },
        "pab_file_move" => FileSystemAction::Move {
            destination: destination()?,
            recursive,
            overwrite,
            limits,
        },
        "pab_file_delete" => FileSystemAction::Delete { recursive, limits },
        "pab_archive_create" => FileSystemAction::ArchiveCreate {
            sources: args
                .get("sources")
                .and_then(Value::as_array)
                .ok_or("sources must be an array")?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "each source must be a path string".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?,
            overwrite,
            limits,
        },
        "pab_archive_extract" => FileSystemAction::ArchiveExtract {
            destination: destination()?,
            overwrite,
            max_ratio: u32::try_from(args.get("max_ratio").and_then(Value::as_u64).unwrap_or(200))
                .map_err(|_| "invalid max_ratio")?,
            limits,
        },
        _ => return Err("unknown bulk filesystem tool".to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn destructive_defaults_limits_and_unknown_fields_are_explicit() {
        assert!(matches!(
            parse("pab_file_copy", &json!({"destination":"/tmp/out"})).unwrap(),
            FileSystemAction::Copy {
                recursive: false,
                overwrite: false,
                ..
            }
        ));
        for args in [
            json!({"device_code":"123456789","path":"/tmp/x","recursive":true,"force":true}),
            json!({"device_code":"123456789","path":"/tmp/x","max_entries":4097}),
        ] {
            assert!(
                super::super::mcp_catalog::validate_arguments("pab_file_delete", &args).is_err()
            );
        }
        let (request, payload) = super::super::mcp_filesystem::parse(
            "pab_archive_create",
            &json!({"path":"/tmp/out.zip","sources":["/tmp/src"]}),
        )
        .unwrap();
        assert!(payload.is_empty());
        assert_eq!(request.operation.schema_version(), 3);
        assert!(request.operation.asynchronous());
        assert!(request.validate().is_ok());
    }
}
