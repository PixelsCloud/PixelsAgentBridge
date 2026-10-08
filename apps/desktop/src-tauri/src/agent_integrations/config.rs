//! Configuration edits are prepared before writing, and never execute YAML tags.
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, value};

pub const PERMISSION: &str = "mcp__pixels__*";
const OWNER: &str = "Pixels Agent Bridge";
const START: &str = "# BEGIN PIXELS AGENT BRIDGE\n";
const END: &str = "# END PIXELS AGENT BRIDGE\n";

pub struct Edit {
    pub path: PathBuf,
    pub before: Option<String>,
    pub after: String,
}

pub fn read(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Could not read {}: {error}", path.display())),
    }
}

pub fn json_document(text: &str) -> Result<Value, String> {
    let text = text.trim_start_matches('\u{feff}');
    let document: Value = if text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(text).map_err(|e| format!("Invalid JSON configuration: {e}"))?
    };
    if !document.is_object() {
        return Err("Configuration must be a JSON object".into());
    }
    Ok(document)
}

pub fn toml_document(text: &str) -> Result<DocumentMut, String> {
    text.trim_start_matches('\u{feff}')
        .parse()
        .map_err(|e| format!("Invalid TOML configuration: {e}"))
}

pub fn json_text(document: &Value) -> String {
    format!(
        "{}\n",
        serde_json::to_string_pretty(document).expect("JSON value")
    )
}

pub fn object_field<'a>(
    parent: &'a mut Value,
    field: &str,
) -> Result<&'a mut serde_json::Map<String, Value>, String> {
    let object = parent
        .as_object_mut()
        .ok_or("Configuration section must be an object")?;
    object
        .entry(field)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("{field} must be an object"))
}

pub fn set_json_server(document: &mut Value, binary: &Path, enabled: bool) -> Result<(), String> {
    let servers = object_field(document, "mcpServers")?;
    if enabled {
        // Replace our own server's launch options, never a third-party entry.
        servers.insert(
            "pixels".into(),
            json!({"type":"stdio", "command":binary, "args":[]}),
        );
    } else {
        servers.remove("pixels");
    }
    Ok(())
}

pub fn set_kimi_permission(document: &mut DocumentMut, enabled: bool) -> Result<(), String> {
    if document.get("permission").is_none() {
        if !enabled {
            return Ok(());
        }
        document["permission"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let permission = document["permission"]
        .as_table_mut()
        .ok_or("permission must be a TOML table")?;
    if permission.get("rules").is_none() {
        if !enabled {
            return Ok(());
        }
        permission["rules"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
    let rules = permission["rules"]
        .as_array_of_tables_mut()
        .ok_or("permission.rules must be an array of tables")?;
    let retained: Vec<_> = rules
        .iter()
        .filter(|rule| {
            !(rule.get("reason").and_then(|v| v.as_str()) == Some(OWNER)
                && rule.get("pattern").and_then(|v| v.as_str()) == Some(PERMISSION))
        })
        .cloned()
        .collect();
    let mut replacement = toml_edit::ArrayOfTables::new();
    if enabled {
        // First-match rules: put the explicit Pixels grant before generic asks.
        let mut rule = toml_edit::Table::new();
        rule["decision"] = value("allow");
        rule["pattern"] = value(PERMISSION);
        rule["reason"] = value(OWNER);
        replacement.push(rule);
    }
    for rule in retained {
        replacement.push(rule);
    }
    *rules = replacement;
    Ok(())
}

pub fn kimi_approved(document: &DocumentMut) -> bool {
    document
        .get("permission")
        .and_then(|p| p.get("rules"))
        .and_then(|r| r.as_array_of_tables())
        .and_then(|rules| rules.iter().next())
        .is_some_and(|rule| {
            rule.get("pattern").and_then(|v| v.as_str()) == Some(PERMISSION)
                && rule.get("decision").and_then(|v| v.as_str()) == Some("allow")
        })
}

pub fn claude_approved(document: &Value) -> bool {
    document
        .pointer("/permissions/allow")
        .and_then(Value::as_array)
        .is_some_and(|rules| rules.iter().any(|r| r.as_str() == Some(PERMISSION)))
        && claude_conflict(document).is_none()
}

pub fn claude_conflict(document: &Value) -> Option<String> {
    for group in ["ask", "deny"] {
        if let Some(rules) = document
            .pointer(&format!("/permissions/{group}"))
            .and_then(Value::as_array)
        {
            if rules.iter().filter_map(Value::as_str).any(|r| {
                super::glob_matches(r, PERMISSION) || r.starts_with("mcp__pixels") || r == "mcp__*"
            }) {
                return Some(format!(
                    "Claude permissions.{group} conflicts with Pixels. Remove that rule in Claude settings first."
                ));
            }
        }
    }
    None
}

// The receipt records only grants inserted by Bridge. Existing grants survive disable.
pub fn set_claude_permission(
    document: &mut Value,
    receipt: &mut Value,
    enabled: bool,
) -> Result<(), String> {
    if enabled && let Some(error) = claude_conflict(document) {
        return Err(error);
    }
    let rules = object_field(document, "permissions")?
        .entry("allow")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or("permissions.allow must be an array")?;
    if enabled {
        if !rules.iter().any(|r| r.as_str() == Some(PERMISSION)) {
            rules.push(json!(PERMISSION));
            receipt["pixelsPermissionAdded"] = json!(true);
        }
    } else {
        if receipt["pixelsPermissionAdded"].as_bool() == Some(true) {
            rules.retain(|r| r.as_str() != Some(PERMISSION));
        }
        receipt["pixelsPermissionAdded"] = json!(false);
    }
    Ok(())
}

pub fn dsh_document(text: &str) -> Result<serde_yaml_ng::Value, String> {
    if text.trim().is_empty() {
        return Ok(serde_yaml_ng::Value::Sequence(vec![]));
    }
    let document: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text).map_err(|e| format!("Invalid DeepSeek YAML: {e}"))?;
    if !document.is_sequence() {
        return Err("DeepSeek patch must be a YAML array".into());
    }
    Ok(document)
}

pub fn dsh_server(text: &str) -> Result<Option<Value>, String> {
    let document = dsh_document(text)?;
    let mut found = None;
    for patch in document.as_sequence().expect("sequence") {
        if patch.get("id").and_then(|v| v.as_str()) == Some("pixels-agent-bridge-mcp")
            || patch
                .get("config")
                .and_then(|v| v.get("serverName"))
                .and_then(|v| v.as_str())
                == Some("pixels")
        {
            return Err("DeepSeek has a separate Pixels override; remove it before managing Bridge integration".into());
        }
        if let Some(rows) = patch.get("insert").and_then(|v| v.as_sequence()) {
            for row in rows {
                if row.get("id").and_then(|v| v.as_str()) == Some("pixels-agent-bridge-mcp")
                    || row
                        .get("config")
                        .and_then(|v| v.get("serverName"))
                        .and_then(|v| v.as_str())
                        == Some("pixels")
                {
                    if found.is_some() {
                        return Err("Duplicate Pixels entries in DeepSeek configuration".into());
                    }
                    // Only project fields used for status; never evaluate !!js values.
                    found = Some(json!({
                        "command":row.get("config").and_then(|v| v.get("command")).and_then(|v| v.as_str()),
                        "args": row.get("config").and_then(|v| v.get("args")).and_then(|v| v.as_sequence()).map(|s| s.len()).unwrap_or(0),
                        "plugin":row.get("name").and_then(|v| v.as_str()),
                    }));
                }
            }
        }
    }
    Ok(found)
}

pub fn set_dsh(text: &str, binary: &Path, enabled: bool) -> Result<String, String> {
    dsh_document(text)?;
    // Own only this marked block; all surrounding YAML/comments/tags stay verbatim.
    let normalized = text.replace("\r\n", "\n");
    let mut retained = normalized.clone();
    match (normalized.find(START), normalized.find(END)) {
        (Some(start), Some(end))
            if start < end
                && normalized.matches(START).count() == 1
                && normalized.matches(END).count() == 1 =>
        {
            retained.replace_range(start..end + END.len(), "");
        }
        (None, None) => {}
        _ => return Err("Invalid Pixels configuration markers in DeepSeek patch".into()),
    }
    if dsh_server(&retained)?.is_some() {
        return Err("DeepSeek already has an unmanaged Pixels entry; remove it before enabling Bridge integration".into());
    }
    if !enabled {
        return Ok(if retained.trim().is_empty() {
            "[]\n".into()
        } else {
            retained
        });
    }
    let parsed = dsh_document(&retained)?;
    if parsed.as_sequence().is_some_and(Vec::is_empty) {
        // Preserve comments in an empty layer, but replace its [] root.
        retained = retained
            .lines()
            .filter(|line| line.trim().starts_with('#'))
            .map(|line| format!("{line}\n"))
            .collect();
    } else if retained.trim_start().starts_with('[') || retained.contains("\n...\n") {
        return Err("DeepSeek patch uses flow/document-end syntax; use a block YAML array before enabling integration".into());
    }
    if !retained.ends_with('\n') {
        retained.push('\n');
    }
    let command = serde_json::to_string(&binary.to_string_lossy()).map_err(|e| e.to_string())?;
    retained.push_str(&format!("{START}- insert:\n    - id: pixels-agent-bridge-mcp\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: pixels\n        transport: stdio\n        command: {command}\n        args: []\n        toolCallTimeoutMs: 180000\n{END}"));
    dsh_document(&retained)?;
    Ok(retained)
}

fn replace(path: &Path, expected: Option<&str>, next: &str) -> Result<(), String> {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!(
            "Configuration is a symbolic link; edit its target directly: {}",
            path.display()
        ));
    }
    if read(path)?.as_deref() != expected {
        return Err(format!(
            "Configuration changed during update: {}. Refresh and retry.",
            path.display()
        ));
    }
    let parent = path
        .parent()
        .ok_or("Configuration has no parent directory")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    if let Ok(metadata) = fs::metadata(path) {
        temp.as_file()
            .set_permissions(metadata.permissions())
            .map_err(|e| e.to_string())?;
    }
    temp.write_all(next.as_bytes()).map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    if read(path)?.as_deref() != expected {
        return Err("Configuration changed during update; refresh and retry".into());
    }
    temp.persist(path)
        .map_err(|e| format!("Could not replace {}: {e}", path.display()))?;
    Ok(())
}

pub fn commit(edits: &[Edit]) -> Result<(), String> {
    for edit in edits {
        if read(&edit.path)? != edit.before {
            return Err("Configuration changed; refresh and retry".into());
        }
    }
    for (index, edit) in edits.iter().enumerate() {
        if edit.before.as_deref() == Some(&edit.after) {
            continue;
        }
        if let Err(error) = replace(&edit.path, edit.before.as_deref(), &edit.after) {
            let mut failures = vec![];
            for prior in edits[..index].iter().rev() {
                let undo = if let Some(before) = &prior.before {
                    replace(&prior.path, Some(&prior.after), before)
                } else if read(&prior.path)?.as_deref() == Some(&prior.after) {
                    fs::remove_file(&prior.path).map_err(|e| e.to_string())
                } else {
                    Err("Configuration changed before rollback".into())
                };
                if let Err(failure) = undo {
                    failures.push(failure);
                }
            }
            return Err(format!(
                "{error}{}",
                if failures.is_empty() {
                    String::new()
                } else {
                    format!("; rollback: {}", failures.join("; "))
                }
            ));
        }
    }
    Ok(())
}
