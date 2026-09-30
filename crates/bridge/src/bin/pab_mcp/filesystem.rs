use pab_bridge::BridgeRuntime;
use pab_protocol::{
    FileSearchMode, FileSystemAction, FileSystemRequest, MAX_TEXT_PAYLOAD_BYTES, RequestId,
    TextEdit, TextEncoding, TextReadRange,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) fn tools() -> Vec<Value> {
    let common = json!({ "device_code": { "type": "string", "pattern": "^[0-9]{9}$" }, "path": { "type": "string", "minLength": 1, "maxLength": 4096 } });
    let mut tools = Vec::new();
    for (name, description, extra, required) in [
        (
            "pab_file_stat",
            "Inspect an absolute target path: type, size, mtime, links and permissions. Does not follow links unless requested. No content or implicit hash scan.",
            json!({ "follow_symlinks": { "type": "boolean", "default": false } }),
            vec!["device_code", "path"],
        ),
        (
            "pab_file_read",
            "Read max 16 KiB text from a file up to 4 MiB. UTF-8 default, auto UTF-8/UTF-16 BOM or explicit encoding. mode=lines uses 1-based start_line/count; mode=bytes uses raw-file offset/max_bytes. Return SHA-256, encoding, newline and next_offset. Continue with expected_hash; offsets must be character boundaries. Reject binary text and symlink/reparse paths.",
            json!({
                "mode": { "type": "string", "enum": ["lines", "bytes"], "default": "lines" },
                "start_line": { "type": "integer", "minimum": 1 }, "count": { "type": "integer", "minimum": 1, "maximum": 500, "default": 100 },
                "offset": { "type": "integer", "minimum": 0 }, "max_bytes": { "type": "integer", "minimum": 4, "maximum": 16384, "default": 16384 },
                "encoding": encoding_schema(), "expected_hash": hash_schema()
            }),
            vec!["device_code", "path"],
        ),
        (
            "pab_file_write",
            "Write UTF-8 input (max 128 KiB), optionally encode UTF-8 BOM or UTF-16 BOM. Explicit overwrite, no parent creation. expected_hash checks existing version and requires overwrite=true. Stage, sync, publish; preserve basic permissions, not custom ACL or ownership. Reuse request_id for deduplication; query pab_get_operation after an unconfirmed result. PAB writers share a path lock; external programs are not an OS-level atomic CAS.",
            json!({
                "content": { "type": "string", "maxLength": 131072 }, "encoding": encoding_schema(),
                "overwrite": { "type": "boolean", "default": false }, "expected_hash": hash_schema(), "request_id": { "type": "string", "format": "uuid" }
            }),
            vec!["device_code", "path", "content"],
        ),
        (
            "pab_file_patch",
            "Apply 1..32 exact nonoverlapping replacements against ORIGINAL text up to 4 MiB. Requires SHA-256 and exact expected_matches (default 1) for each find. Conflicts do not publish. Preserve original encoding/BOM and untouched newlines. Max 128 KiB serialized edits. Reuse request_id; query original operation after an unconfirmed result.",
            json!({
            "expected_hash": hash_schema(), "encoding": encoding_schema(), "request_id": { "type": "string", "format": "uuid" },
                "edits": { "type": "array", "minItems": 1, "maxItems": 32, "items": { "type": "object", "properties": {
                    "find": { "type": "string", "minLength": 1, "maxLength": 131072 }, "replace": { "type": "string", "maxLength": 131072 },
                    "expected_matches": { "type": "integer", "minimum": 1, "maximum": 1000, "default": 1 }
                }, "required": ["find", "replace"], "additionalProperties": false } }
            }),
            vec!["device_code", "path", "expected_hash", "edits"],
        ),
    ] {
        let mut properties = common.clone();
        properties
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        tools.push(json!({ "name": name, "description": description, "inputSchema": { "type": "object", "properties": properties, "required": required, "additionalProperties": false }, "annotations": { "readOnlyHint": matches!(name, "pab_file_stat" | "pab_file_read"), "destructiveHint": matches!(name, "pab_file_write" | "pab_file_patch"), "idempotentHint": matches!(name, "pab_file_stat" | "pab_file_read"), "openWorldHint": false } }));
    }
    let mut add = |name: &str,
                   description: &str,
                   extra: Value,
                   required: Vec<&str>,
                   read_only: bool| {
        let mut properties = common.clone();
        properties
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        tools.push(json!({ "name": name, "description": description, "inputSchema": { "type": "object", "properties": properties, "required": required, "additionalProperties": false }, "annotations": { "readOnlyHint": read_only, "destructiveHint": false, "idempotentHint": read_only, "openWorldHint": false } }));
    };
    add(
        "pab_file_search",
        "Search a directory by literal filename substring or literal text. Glob matches relative paths with / separators, ** recursive; includes hidden files and does not apply gitignore. case_sensitive defaults true. No link traversal. Up to 100 matches, 4096 entries, 64 MiB scanned, 5 seconds; truncated/skip warnings are explicit. Content is strict UTF-8 or UTF-16 BOM, max 4 MiB/file; previews 160 characters. Results are a bounded traversal, not a snapshot or pageable. Requires filesystem v2.",
        json!({ "mode": { "type":"string", "enum":["name", "content"], "default":"name" }, "query": { "type":"string", "minLength":1, "maxLength":1024 }, "glob": { "type":"string", "minLength":1, "maxLength":1024, "default":"**/*" }, "case_sensitive": { "type":"boolean", "default":true }, "max_results": { "type":"integer", "minimum":1, "maximum":100, "default":100 }, "max_depth": { "type":"integer", "minimum":1, "maximum":64, "default":16 }, "max_file_bytes": { "type":"integer", "minimum":1, "maximum":4194304, "default":4194304 } }),
        vec!["device_code", "path", "query"],
        true,
    );
    add(
        "pab_file_hash",
        "Start asynchronous streaming SHA-256 of an ordinary file, including binary and large files. Return operation_ref after remote acceptance; query pab_get_operation for byte progress/result and pab_cancel_operation to stop. Do not follow links. Detect observed size/mtime/identity changes; no atomic external-write snapshot. Max 4 Executor hash jobs, 30 minutes/job, 30 seconds/read. Reuse request_id without restarting the operation. Requires filesystem v2.",
        json!({ "request_id": { "type":"string", "format":"uuid" } }),
        vec!["device_code", "path"],
        true,
    );
    add(
        "pab_mkdir",
        "Create a directory with explicit parents=false and exist_ok=false defaults. Absolute normalized path, no symlink/reparse traversal. Report paths actually created, including partial failure; no rollback. Reuse request_id; after an uncertain reply query the original operation, never replay. Up to 128 components, bounded created-path report. Requires filesystem v2.",
        json!({ "parents": { "type":"boolean", "default":false }, "exist_ok": { "type":"boolean", "default":false }, "request_id": { "type":"string", "format":"uuid" } }),
        vec!["device_code", "path"],
        false,
    );
    tools.extend(super::mcp_filesystem_bulk::tools());
    tools
}

fn encoding_schema() -> Value {
    json!({ "type": "string", "enum": ["utf8", "utf8_bom", "utf16_le", "utf16_be"] })
}
fn hash_schema() -> Value {
    json!({ "type": "string", "minLength": 64, "maxLength": 64 })
}

pub(super) fn parse(name: &str, args: &Value) -> Result<(FileSystemRequest, Vec<u8>), String> {
    let encoding = args
        .get("encoding")
        .map(|value| serde_json::from_value::<TextEncoding>(value.clone()))
        .transpose()
        .map_err(|error| error.to_string())?;
    let expected_hash = args
        .get("expected_hash")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut payload = Vec::new();
    let operation = match name {
        "pab_file_copy"
        | "pab_file_move"
        | "pab_file_delete"
        | "pab_archive_create"
        | "pab_archive_extract" => super::mcp_filesystem_bulk::parse(name, args)?,
        "pab_file_search" => FileSystemAction::Search {
            mode: serde_json::from_value::<FileSearchMode>(
                args.get("mode").cloned().unwrap_or(json!("name")),
            )
            .map_err(|e| e.to_string())?,
            query: args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("query must be text")?
                .to_owned(),
            glob: args
                .get("glob")
                .and_then(Value::as_str)
                .unwrap_or("**/*")
                .to_owned(),
            case_sensitive: args
                .get("case_sensitive")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            max_results: args
                .get("max_results")
                .and_then(Value::as_u64)
                .unwrap_or(100) as u32,
            max_depth: args.get("max_depth").and_then(Value::as_u64).unwrap_or(16) as u32,
            max_file_bytes: args
                .get("max_file_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(4194304) as u32,
        },
        "pab_file_hash" => FileSystemAction::Hash,
        "pab_mkdir" => FileSystemAction::Mkdir {
            parents: args
                .get("parents")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            exist_ok: args
                .get("exist_ok")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "pab_file_stat" => FileSystemAction::Stat {
            follow_symlinks: args
                .get("follow_symlinks")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "pab_file_read" => {
            let range = match args.get("mode").and_then(Value::as_str).unwrap_or("lines") {
                "lines" => {
                    if args.get("offset").is_some() || args.get("max_bytes").is_some() {
                        return Err("offset/max_bytes require mode=bytes".to_owned());
                    }
                    TextReadRange::Lines {
                        start_line: args.get("start_line").and_then(Value::as_u64).unwrap_or(1),
                        count: args.get("count").and_then(Value::as_u64).unwrap_or(100) as u32,
                    }
                }
                "bytes" => {
                    if args.get("start_line").is_some() || args.get("count").is_some() {
                        return Err("start_line/count require mode=lines".to_owned());
                    }
                    TextReadRange::Bytes {
                        offset: args.get("offset").and_then(Value::as_u64).unwrap_or(0),
                        max_bytes: args
                            .get("max_bytes")
                            .and_then(Value::as_u64)
                            .unwrap_or(16384) as u32,
                    }
                }
                _ => return Err("mode must be lines or bytes".to_owned()),
            };
            FileSystemAction::Read {
                range,
                encoding,
                expected_hash,
            }
        }
        "pab_file_write" => {
            payload = args
                .get("content")
                .and_then(Value::as_str)
                .ok_or("content must be text")?
                .as_bytes()
                .to_vec();
            FileSystemAction::Write {
                encoding: encoding.unwrap_or(TextEncoding::Utf8),
                overwrite: args
                    .get("overwrite")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                expected_hash,
            }
        }
        "pab_file_patch" => {
            let edits = args
                .get("edits")
                .and_then(Value::as_array)
                .ok_or("edits must be an array")?
                .iter()
                .map(|edit| {
                    Ok(TextEdit {
                        find: edit
                            .get("find")
                            .and_then(Value::as_str)
                            .ok_or("find must be text")?
                            .to_owned(),
                        replace: edit
                            .get("replace")
                            .and_then(Value::as_str)
                            .ok_or("replace must be text")?
                            .to_owned(),
                        expected_matches: edit
                            .get("expected_matches")
                            .and_then(Value::as_u64)
                            .unwrap_or(1) as u32,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            if edits.is_empty() || edits.len() > 32 {
                return Err("provide 1..32 edits".to_owned());
            }
            payload = serde_json::to_vec(&edits).map_err(|error| error.to_string())?;
            FileSystemAction::Patch {
                expected_hash: expected_hash.ok_or("expected_hash is required")?,
                encoding,
            }
        }
        _ => return Err("unknown filesystem tool".to_owned()),
    };
    if payload.len() > MAX_TEXT_PAYLOAD_BYTES {
        return Err("UTF-8 payload exceeds 128 KiB; use file transfer".to_owned());
    }
    let request = FileSystemRequest {
        request_id: args
            .get("request_id")
            .and_then(Value::as_str)
            .map(str::parse)
            .transpose()
            .map_err(|_| "invalid request_id")?
            .unwrap_or_else(RequestId::new),
        path: args
            .get("path")
            .and_then(Value::as_str)
            .ok_or("path must be text")?
            .to_owned(),
        payload_size: payload.len() as u32,
        payload_sha256: operation
            .has_payload()
            .then(|| format!("{:x}", Sha256::digest(&payload))),
        operation,
    };
    request.validate().map_err(str::to_owned)?;
    Ok((request, payload))
}

pub(super) async fn call(
    runtime: &BridgeRuntime,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    let (request, payload) = parse(name, args)?;
    let device = super::mcp_tools::resolve_target(runtime, args).await?;
    let target = runtime
        .current_environment(device)
        .await
        .map_err(|error| error.to_string())?;
    let result = runtime
        .filesystem(device, &request, &payload)
        .await
        .map_err(|error| error.to_string())?;
    let text = if name == "pab_file_read" && result.reply.state == "completed" {
        Some(String::from_utf8(result.data).map_err(|error| error.to_string())?)
    } else {
        None
    };
    Ok(
        json!({ "device_ref": device, "operation_ref": { "device_code": args["device_code"], "operation_id": request.request_id, "kind": request.operation.kind() }, "result": result.reply, "text": text, "os_reminder": target.compact_reminder() }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn b2_tools_validate_limits_and_mkdir_never_requires_a_binary_payload() {
        let (_, payload) = parse("pab_mkdir", &json!({ "path":"/tmp/new" })).unwrap();
        assert!(payload.is_empty());
        for args in [
            json!({"device_code":"123456789","path":"/tmp/root","query":"x","max_results":101}),
            json!({"device_code":"123456789","path":"/tmp/root","query":"x","max_depth":0}),
            json!({"device_code":"123456789","path":"/tmp/root","query":"x","regex":true}),
        ] {
            assert!(
                super::super::mcp_catalog::validate_arguments("pab_file_search", &args).is_err()
            );
        }
        let (request, _) =
            parse("pab_file_search", &json!({"path":"/tmp/root","query":"x"})).unwrap();
        assert!(matches!(
            request.operation,
            FileSystemAction::Search {
                mode: FileSearchMode::Name,
                case_sensitive: true,
                max_results: 100,
                ..
            }
        ));
        let (request, _) = parse(
            "pab_file_hash",
            &json!({"path":"/tmp/binary","request_id": RequestId::from_u128(123).to_string()}),
        )
        .unwrap();
        assert_eq!(request.request_id, RequestId::from_u128(123));
        assert!(request.payload_sha256.is_none());
    }
    #[test]
    fn ranges_payload_bytes_and_patch_defaults_are_checked() {
        let invalid = json!({"device_code":"123456789","path":"/tmp/x","expected_hash":"a".repeat(64),"edits":[]});
        assert!(super::super::mcp_catalog::validate_arguments("pab_file_patch", &invalid).is_err());
        let invalid = json!({"device_code":"123456789","path":"/tmp/x","expected_hash":"a".repeat(64),"edits":[{"find":"a","replace":"b","command":"injected"}]});
        assert!(super::super::mcp_catalog::validate_arguments("pab_file_patch", &invalid).is_err());
        assert!(
            parse(
                "pab_file_read",
                &json!({"path":"/tmp/x","mode":"bytes","start_line":1})
            )
            .is_err()
        );
        assert!(parse("pab_file_read", &json!({"path":"/tmp/x","offset":0})).is_err());
        assert!(
            parse(
                "pab_file_write",
                &json!({"path":"/tmp/x","content":"中".repeat(50000)})
            )
            .is_err()
        );
        let (request, payload) = parse("pab_file_patch", &json!({"path":"/tmp/x","expected_hash":"a".repeat(64),"edits":[{"find":"a","replace":"b"}]})).unwrap();
        assert_eq!(request.payload_size as usize, payload.len());
        assert_eq!(
            serde_json::from_slice::<Vec<TextEdit>>(&payload).unwrap()[0].expected_matches,
            1
        );
    }
}
