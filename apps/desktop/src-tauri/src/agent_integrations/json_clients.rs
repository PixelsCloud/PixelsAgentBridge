use super::{AgentId, Location, config, edit, location_in};
use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};
use serde_json::{Value, json};
use std::{
    env,
    path::{Path, PathBuf},
};

pub(super) fn opencode_location(home: &Path) -> Result<Location, String> {
    let root = env::var_os("OPENCODE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_CONFIG_HOME")
                .filter(|v| !v.is_empty())
                .map(|p| PathBuf::from(p).join("opencode"))
        })
        .unwrap_or_else(|| home.join(".config/opencode"));
    let mut paths = location_in(AgentId::Opencode, home, Some(root));
    if env::var_os("OPENCODE_CONFIG_DIR").is_none_or(|v| v.is_empty())
        && let Some(file) = env::var_os("OPENCODE_CONFIG").filter(|v| !v.is_empty())
    {
        paths.config = PathBuf::from(file);
        if !paths.config.is_absolute() {
            return Err("OPENCODE_CONFIG must be an absolute path for desktop integration".into());
        }
        paths.receipt = paths
            .config
            .with_extension("pixels-bridge-integration.json");
    }
    Ok(paths)
}

fn root(text: &str) -> Result<CstRootNode, String> {
    let node = CstRootNode::parse(text.trim_start_matches('\u{feff}'), &Default::default())
        .map_err(|e| format!("Invalid JSONC configuration: {e}"))?;
    if node.value().is_none() {
        node.set_value(CstInputValue::Object(vec![]));
    }
    let object = node
        .object_value()
        .ok_or("Configuration must be an object")?;
    check_keys(&object)?;
    Ok(node)
}

fn check_keys(object: &CstObject) -> Result<(), String> {
    let mut names = std::collections::HashSet::new();
    for prop in object.properties() {
        let name = prop.decoded_name().ok_or("Invalid JSONC property")?;
        if !names.insert(name) {
            return Err(
                "Duplicate configuration property; remove duplicates before enabling integration"
                    .into(),
            );
        }
        if let Some(nested) = prop.object_value() {
            check_keys(&nested)?;
        }
    }
    Ok(())
}

fn data(node: &CstRootNode) -> Result<Value, String> {
    jsonc_parser::parse_to_serde_value(&node.to_string(), &Default::default())
        .map_err(|e| e.to_string())
}

fn input(value: &Value) -> CstInputValue {
    match value {
        Value::Null => CstInputValue::Null,
        Value::Bool(v) => (*v).into(),
        Value::String(v) => v.clone().into(),
        Value::Number(v) => CstInputValue::Number(v.to_string()),
        Value::Array(v) => CstInputValue::Array(v.iter().map(input).collect()),
        Value::Object(v) => {
            CstInputValue::Object(v.iter().map(|(k, v)| (k.clone(), input(v))).collect())
        }
    }
}

fn put(object: &CstObject, key: &str, value: &Value) {
    if let Some(prop) = object.get(key) {
        prop.set_value(input(value));
    } else {
        object.append(key, input(value));
    }
}

fn object(parent: &CstObject, key: &str) -> Result<CstObject, String> {
    parent
        .object_value_or_create(key)
        .ok_or_else(|| format!("{key} must be an object"))
}

pub(super) fn inspect(paths: &Location, text: &str) -> Result<(Option<Value>, bool), String> {
    check_opencode_layers(paths)?;
    let doc = root(text)?;
    let data = data(&doc)?;
    check_opencode_tools(&data)?;
    if data.get("mcp").is_some_and(|v| !v.is_object()) {
        return Err("mcp must be an object".into());
    }
    let mut entry = data.pointer("/mcp/pixels").cloned();
    // Shared ownership checks expect an executable plus a separate argv.
    if let Some(entry) = entry.as_mut() {
        let original = entry.clone();
        let command = original["command"].as_array();
        *entry = json!({"command":command.and_then(|v|v.first()),"args":command.map(|v|v.len().saturating_sub(1)).unwrap_or(1),"enabled":original["enabled"]});
        if original["type"] != "local" {
            entry["url"] = json!("non-local");
        }
    }
    let approved = doc
        .object_value()
        .and_then(|o| o.object_value("permission"))
        .and_then(|p| p.properties().last().cloned())
        .is_some_and(|p| {
            p.decoded_name().as_deref() == Some("pixels_*")
                && data["permission"]["pixels_*"] == "allow"
        });
    Ok((entry, approved))
}

fn check_opencode_layers(paths: &Location) -> Result<(), String> {
    let Some(parent) = paths.config.parent() else {
        return Ok(());
    };
    for name in ["config.json", "opencode.json", "opencode.jsonc"] {
        let sibling = parent.join(name);
        if sibling == paths.config {
            continue;
        }
        if let Some(text) = config::read(&sibling)? {
            if data(&root(&text)?)?.pointer("/mcp/pixels").is_some() {
                return Err(format!(
                    "Pixels is also configured in {}; consolidate the entry before changing integration",
                    sibling.display()
                ));
            }
        }
    }
    Ok(())
}

fn check_opencode_tools(doc: &Value) -> Result<(), String> {
    // Legacy filters can hide tools even when the permission grant is present.
    if doc
        .get("tools")
        .and_then(Value::as_object)
        .is_some_and(|rules| {
            rules.iter().any(|(key, value)| {
                value == false
                    && (key.starts_with("pixels_") || super::glob_matches(key, "pixels_*"))
            })
        })
    {
        return Err(
            "OpenCode tools disables Pixels tools; remove the conflicting filter first".into(),
        );
    }
    Ok(())
}

pub(super) fn prepare(
    paths: &Location,
    binary: &Path,
    enabled: bool,
) -> Result<Vec<config::Edit>, String> {
    let receipt_before = config::read(&paths.receipt)?;
    let mut receipt = config::json_document(receipt_before.as_deref().unwrap_or_default())?;
    let mut edits = Vec::new();
    edits.push(edit(&paths.config, |text| {
        let doc = root(text)?;
        let top = doc.object_value().unwrap();
        let servers = object(&top, "mcp")?;
        if enabled {
            let value = json!({"type":"local","command":[binary],"enabled":true,"timeout":180000});
            put(&servers, "pixels", &value);
        } else if let Some(prop) = servers.get("pixels") {
            prop.remove();
        }
        opencode_permission(&doc, &mut receipt, enabled)?;
        Ok(doc.to_string())
    })?);
    edits.push(config::Edit {
        path: paths.receipt.clone(),
        before: receipt_before,
        after: config::json_text(&receipt),
    });
    Ok(edits)
}

fn opencode_permission(
    doc: &CstRootNode,
    receipt: &mut Value,
    enabled: bool,
) -> Result<(), String> {
    let top = doc.object_value().unwrap();
    let original = data(doc)?;
    if enabled {
        if let Some(value) = original.get("permission").filter(|v| !v.is_object()) {
            let value = value
                .as_str()
                .filter(|v| ["allow", "ask", "deny"].contains(v))
                .ok_or("OpenCode permission must be an object or allow/ask/deny")?;
            receipt["opencodeScalarPermission"] = json!(value);
            put(&top, "permission", &json!({"*":value}));
        }
        let permission = object(&top, "permission")?;
        let props = permission.properties();
        if receipt.get("opencodePreviousGrant").is_none() {
            receipt["opencodePreviousGrant"] = json!({"value":original.pointer("/permission/pixels_*"),"index":props.iter().position(|p|p.decoded_name().as_deref()==Some("pixels_*"))});
        }
        // OpenCode applies the last matching permission. Append only the Pixels grant.
        if props.last().and_then(|p| p.decoded_name()).as_deref() != Some("pixels_*")
            || original.pointer("/permission/pixels_*") != Some(&json!("allow"))
        {
            if let Some(prop) = permission.get("pixels_*") {
                prop.remove();
            }
            permission.append("pixels_*", "allow".into());
        }
    } else if let Some(previous) = receipt.get("opencodePreviousGrant").cloned() {
        if original.pointer("/permission/pixels_*") == Some(&json!("allow")) {
            let permission = object(&top, "permission")?;
            if let Some(prop) = permission.get("pixels_*") {
                prop.remove();
            }
            if !previous["value"].is_null() {
                let index = previous["index"].as_u64().unwrap_or(0) as usize;
                permission.insert(
                    index.min(permission.properties().len()),
                    "pixels_*",
                    input(&previous["value"]),
                );
            }
            if let Some(scalar) = receipt.get("opencodeScalarPermission") {
                if data(doc)?["permission"] == json!({"*":scalar}) {
                    put(&top, "permission", scalar);
                }
            }
        }
        receipt
            .as_object_mut()
            .unwrap()
            .remove("opencodePreviousGrant");
        receipt
            .as_object_mut()
            .unwrap()
            .remove("opencodeScalarPermission");
    }
    Ok(())
}
