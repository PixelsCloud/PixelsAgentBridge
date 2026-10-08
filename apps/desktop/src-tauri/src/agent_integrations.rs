use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use toml_edit::{DocumentMut, value};

mod config;
mod discovery;
mod json_clients;
#[cfg(test)]
use std::fs;
#[cfg(test)]
mod integration_tests;

static UPDATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentId {
    Codex,
    Kimi,
    Claude,
    Deepseek,
    Opencode,
}

impl AgentId {
    #[cfg(test)]
    const ALL: [Self; 5] = [
        Self::Codex,
        Self::Kimi,
        Self::Claude,
        Self::Deepseek,
        Self::Opencode,
    ];
    fn executable(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Kimi => "kimi",
            Self::Claude => "claude",
            Self::Deepseek => "dsh",
            Self::Opencode => "opencode",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntegration {
    id: AgentId,
    detected: bool,
    mcp_available: bool,
    configured: bool,
    enabled: bool,
    conflict: bool,
    config_path: String,
    error: Option<String>,
}

struct Location {
    config: PathBuf,
    permissions: PathBuf,
    receipt: PathBuf,
}

fn user_home() -> Result<PathBuf, String> {
    #[cfg(windows)]
    let names = ["USERPROFILE", "HOME"];
    #[cfg(not(windows))]
    let names = ["HOME", "USERPROFILE"];
    names
        .iter()
        .find_map(env::var_os)
        .map(PathBuf::from)
        .ok_or("User home directory is unknown".into())
}

fn location_in(id: AgentId, home: &Path, custom: Option<PathBuf>) -> Location {
    let root = custom.clone().unwrap_or_else(|| {
        home.join(match id {
            AgentId::Codex => ".codex",
            AgentId::Kimi => ".kimi-code",
            AgentId::Claude => ".claude",
            AgentId::Deepseek => ".dsh",
            AgentId::Opencode => ".config/opencode",
        })
    });
    let config = match id {
        AgentId::Codex => root.join("config.toml"),
        AgentId::Kimi => root.join("mcp.json"),
        AgentId::Claude if custom.is_none() => home.join(".claude.json"),
        AgentId::Claude => root.join(".claude.json"),
        AgentId::Deepseek => root.join("cordis.patch.yml"),
        AgentId::Opencode => ["opencode.jsonc", "opencode.json", "config.json"]
            .iter()
            .map(|name| root.join(name))
            .find(|path| path.is_file())
            .unwrap_or_else(|| root.join("opencode.jsonc")),
    };
    Location {
        config,
        permissions: root.join(if id == AgentId::Kimi {
            "config.toml"
        } else {
            "settings.json"
        }),
        receipt: root.join("pixels-bridge-integration.json"),
    }
}

fn location(id: AgentId) -> Result<Location, String> {
    if id == AgentId::Opencode {
        return json_clients::opencode_location(&user_home()?);
    }
    let name = match id {
        AgentId::Codex => "CODEX_HOME",
        AgentId::Kimi => "KIMI_CODE_HOME",
        AgentId::Claude => "CLAUDE_CONFIG_DIR",
        AgentId::Deepseek => "DSH_HOME",
        AgentId::Opencode => unreachable!(),
    };
    Ok(location_in(
        id,
        &user_home()?,
        env::var_os(name)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from),
    ))
}

fn mcp_binary() -> Result<PathBuf, String> {
    let executable = env::current_exe().map_err(|e| e.to_string())?;
    let directory = executable
        .parent()
        .ok_or("Application has no parent directory")?;
    let name = if cfg!(windows) {
        "pab-mcp.exe"
    } else {
        "run-mcp.sh"
    };
    let binary = directory.join(name);
    #[cfg(target_os = "macos")]
    if !binary.is_file() && directory.ends_with("Contents/MacOS") {
        return Ok(PathBuf::from(
            "/Library/Application Support/PixelsAgentBridge/run-mcp.sh",
        ));
    }
    Ok(binary)
}

fn agent_binary(id: AgentId) -> Option<PathBuf> {
    let mut directories: Vec<PathBuf> = env::var_os("PATH")
        .map(|v| env::split_paths(&v).collect())
        .unwrap_or_default();
    if let Ok(home) = user_home() {
        for suffix in [
            ".local/bin",
            ".kimi-code/bin",
            ".claude/local",
            ".cargo/bin",
            ".opencode/bin",
        ] {
            directories.push(home.join(suffix));
        }
        #[cfg(windows)]
        directories.push(home.join("AppData/Roaming/npm"));
    }
    #[cfg(windows)]
    if let Some(appdata) = env::var_os("APPDATA") {
        directories.push(PathBuf::from(appdata).join("npm"));
    }
    #[cfg(not(windows))]
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    if let Some(prefix) = env::var_os("npm_config_prefix").filter(|p| !p.is_empty()) {
        let prefix = PathBuf::from(prefix);
        directories.push(if cfg!(windows) {
            prefix
        } else {
            prefix.join("bin")
        });
    }
    discovery::find_agent(
        id,
        &directories,
        &discovery::npm_caches(user_home().ok().as_deref()),
    )
}

fn find_agent_launcher(id: AgentId, directories: &[PathBuf]) -> Option<PathBuf> {
    for directory in directories {
        for extension in if cfg!(windows) {
            &[".exe", ".cmd"][..]
        } else {
            &[""][..]
        } {
            let path = directory.join(format!("{}{extension}", id.executable()));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ if cfg!(windows) => a
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy()),
        _ => a == b,
    }
}

fn pixels_entry(entry: &Value) -> bool {
    let Some(command) = entry.get("command").and_then(Value::as_str) else {
        return false;
    };
    let name = Path::new(command)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    ["pab-mcp.exe", "pab-mcp", "run-mcp.sh"]
        .iter()
        .any(|n| name.eq_ignore_ascii_case(n))
        && entry.get("url").is_none()
        && entry.get("args").is_none_or(|args| {
            args.as_array().is_some_and(Vec::is_empty) || args.as_u64() == Some(0)
        })
}

fn entry_matches(entry: &Value, binary: &Path) -> bool {
    pixels_entry(entry)
        && entry
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|p| same_path(Path::new(p), binary))
        && entry.get("enabled").and_then(Value::as_bool) != Some(false)
        && entry.get("disabled").and_then(Value::as_bool) != Some(true)
}

fn inspect(
    id: AgentId,
    paths: &Location,
    binary: &Path,
    detected: bool,
) -> Result<AgentIntegration, String> {
    let text = config::read(&paths.config)?.unwrap_or_default();
    let (entry, approved) = match id {
        AgentId::Codex => {
            let doc = config::toml_document(&text)?;
            let entry = doc.get("mcp_servers").and_then(|s| s.get("pixels")).map(|p| json!({
                "command": p.get("command").and_then(|v| v.as_str()),
                "enabled": p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                "args": p.get("args").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
                "url": p.get("url").and_then(|v| v.as_str()),
            }));
            // Remove absent url so the ownership check can distinguish HTTP entries.
            let entry = entry.map(|mut e| {
                if e["url"].is_null() {
                    e.as_object_mut().unwrap().remove("url");
                }
                e
            });
            (entry, pixels_policy_is_approved_in(&doc))
        }
        AgentId::Kimi | AgentId::Claude => {
            let doc = config::json_document(&text)?;
            if doc.get("mcpServers").is_some_and(|s| !s.is_object()) {
                return Err("mcpServers must be an object".into());
            }
            let permission_text = config::read(&paths.permissions)?.unwrap_or_default();
            let approved = if id == AgentId::Kimi {
                config::kimi_approved(&config::toml_document(&permission_text)?)
            } else {
                config::claude_approved(&config::json_document(&permission_text)?)
            };
            (doc.pointer("/mcpServers/pixels").cloned(), approved)
        }
        AgentId::Deepseek => (config::dsh_server(&text)?, true),
        AgentId::Opencode => json_clients::inspect(paths, &text)?,
    };
    let conflict = entry.as_ref().is_some_and(|e| {
        !pixels_entry(e)
            || (id == AgentId::Deepseek
                && (e["plugin"] != "@deepseek-ai/dsh-mcp-client"
                    || !text
                        .replace("\r\n", "\n")
                        .contains("# BEGIN PIXELS AGENT BRIDGE\n")))
    });
    Ok(AgentIntegration {
        id,
        detected,
        mcp_available: binary.is_file(),
        configured: entry.is_some(),
        enabled: !conflict && approved && entry.as_ref().is_some_and(|e| entry_matches(e, binary)),
        conflict,
        config_path: paths.config.display().to_string(),
        error: None,
    })
}

async fn kimi_version(binary: &Path) -> Result<(), String> {
    let mut command = tokio::process::Command::new(binary);
    command
        .arg("--version")
        .kill_on_drop(true)
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000);
    }
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .map_err(|_| "Kimi version check timed out")?
        .map_err(|e| e.to_string())?;
    let version = String::from_utf8_lossy(&output.stdout);
    if !output.status.success()
        || !version
            .split(|c: char| !c.is_ascii_digit() && c != '.')
            .any(|v| {
                v.split('.')
                    .next()
                    .and_then(|s| s.parse::<u32>().ok())
                    .is_some_and(|major| major >= 2)
            })
    {
        return Err("Kimi Code 2.x or later is required; legacy kimi-cli is not configured".into());
    }
    Ok(())
}

async fn status(id: AgentId) -> AgentIntegration {
    let binary = agent_binary(id);
    let result = (|| {
        let paths = location(id)?;
        inspect(id, &paths, &mcp_binary()?, binary.is_some())
    })();
    let mut status = result.unwrap_or_else(|error| AgentIntegration {
        id,
        detected: binary.is_some(),
        mcp_available: mcp_binary().is_ok_and(|p| p.is_file()),
        configured: false,
        enabled: false,
        conflict: false,
        config_path: location(id)
            .map(|p| p.config.display().to_string())
            .unwrap_or_default(),
        error: Some(error),
    });
    if id == AgentId::Kimi
        && let Some(binary) = binary
        && let Err(error) = kimi_version(&binary).await
    {
        status.error = Some(error);
        status.enabled = false;
    }
    status
}

fn edit(
    path: &Path,
    transform: impl FnOnce(&str) -> Result<String, String>,
) -> Result<config::Edit, String> {
    let before = config::read(path)?;
    let after = transform(before.as_deref().unwrap_or_default())?;
    Ok(config::Edit {
        path: path.to_owned(),
        before,
        after,
    })
}

fn prepare(
    id: AgentId,
    paths: &Location,
    binary: &Path,
    enabled: bool,
) -> Result<Vec<config::Edit>, String> {
    let state = inspect(id, paths, binary, true)?;
    if state.conflict {
        return Err("Another or unmanaged MCP entry already uses the pixels name".into());
    }
    let mut edits = vec![];
    match id {
        AgentId::Codex => edits.push(edit(&paths.config, |text| {
            let mut doc = config::toml_document(text)?;
            if enabled {
                if doc.get("mcp_servers").is_none() {
                    doc["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
                }
                let servers = doc["mcp_servers"]
                    .as_table_mut()
                    .ok_or("mcp_servers must be a table")?;
                if !servers.contains_key("pixels") {
                    servers["pixels"] = toml_edit::Item::Table(toml_edit::Table::new());
                }
                let pixels = servers["pixels"]
                    .as_table_mut()
                    .ok_or("pixels must be a table")?;
                pixels["command"] = value(binary.to_string_lossy().as_ref());
                pixels["enabled"] = value(true);
                pixels.remove("args");
                allow_all_pixels_tools_in(&mut doc)?;
            } else if let Some(servers) = doc.get_mut("mcp_servers").and_then(|s| s.as_table_mut())
            {
                servers.remove("pixels");
            }
            Ok(doc.to_string())
        })?),
        AgentId::Kimi | AgentId::Claude => {
            if id == AgentId::Kimi {
                edits.push(edit(&paths.permissions, |text| {
                    let mut doc = config::toml_document(text)?;
                    config::set_kimi_permission(&mut doc, enabled)?;
                    Ok(doc.to_string())
                })?);
            } else {
                let receipt_before = config::read(&paths.receipt)?;
                let mut receipt =
                    config::json_document(receipt_before.as_deref().unwrap_or_default())?;
                edits.push(edit(&paths.permissions, |text| {
                    let mut doc = config::json_document(text)?;
                    config::set_claude_permission(&mut doc, &mut receipt, enabled)?;
                    Ok(config::json_text(&doc))
                })?);
                edits.push(config::Edit {
                    path: paths.receipt.clone(),
                    before: receipt_before,
                    after: config::json_text(&receipt),
                });
            }
            edits.push(edit(&paths.config, |text| {
                let mut doc = config::json_document(text)?;
                config::set_json_server(&mut doc, binary, enabled)?;
                if id == AgentId::Kimi && enabled {
                    doc["mcpServers"]["pixels"]
                        .as_object_mut()
                        .unwrap()
                        .remove("type");
                    doc["mcpServers"]["pixels"]["toolTimeoutMs"] = json!(180000);
                }
                Ok(config::json_text(&doc))
            })?);
        }
        AgentId::Deepseek => edits.push(edit(&paths.config, |text| {
            config::set_dsh(text, binary, enabled)
        })?),
        AgentId::Opencode => edits.extend(json_clients::prepare(paths, binary, enabled)?),
    }
    Ok(edits)
}

#[tauri::command]
pub async fn agent_integration_status(id: AgentId) -> AgentIntegration {
    status(id).await
}

#[tauri::command]
pub async fn set_agent_integration(id: AgentId, enabled: bool) -> Result<AgentIntegration, String> {
    let _guard = UPDATE_LOCK.lock().await;
    let paths = location(id)?;
    let mcp = mcp_binary()?;
    if enabled {
        let agent = agent_binary(id).ok_or("AI client executable was not found")?;
        if id == AgentId::Kimi {
            kimi_version(&agent).await?;
        }
        if !mcp.is_file() {
            return Err("The installed MCP executable is missing".into());
        }
    }
    // Validate every configuration before starting the probe or changing files.
    let edits = prepare(id, &paths, &mcp, enabled)?;
    if enabled {
        tokio::time::timeout(Duration::from_secs(12), verify_mcp(&mcp))
            .await
            .map_err(|_| "MCP verification timed out")??;
    }
    config::commit(&edits)?;
    let updated = status(id).await;
    if updated.enabled != enabled || updated.error.is_some() {
        return Err(updated
            .error
            .unwrap_or("Configuration verification failed".into()));
    }
    Ok(updated)
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let (p, v) = (pattern.as_bytes(), value.as_bytes());
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < v.len() {
        if i < p.len() && p[i] == v[j] {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some(i);
            i += 1;
            mark = j;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == b'*' {
        i += 1;
    }
    i == p.len()
}

fn pixels_policy_is_approved_in(document: &DocumentMut) -> bool {
    let Some(pixels) = document
        .get("mcp_servers")
        .and_then(|servers| servers.get("pixels"))
    else {
        return false;
    };
    let defaults_to_approved = pixels
        .get("default_tools_approval_mode")
        .and_then(|mode| mode.as_str())
        == Some("approve");
    let all_tools_visible =
        pixels.get("enabled_tools").is_none() && pixels.get("disabled_tools").is_none();
    let overrides_are_approved = pixels
        .get("tools")
        .and_then(|tools| tools.as_table_like())
        .is_none_or(|tools| {
            tools.iter().all(|(_, tool)| {
                tool.get("approval_mode")
                    .and_then(|mode| mode.as_str())
                    .is_none_or(|mode| mode == "approve")
            })
        });
    defaults_to_approved && all_tools_visible && overrides_are_approved
}

fn allow_all_pixels_tools_in(document: &mut DocumentMut) -> Result<(), String> {
    let pixels = document
        .get_mut("mcp_servers")
        .and_then(|servers| servers.get_mut("pixels"))
        .and_then(|pixels| pixels.as_table_mut())
        .ok_or("Pixels is missing from the Codex MCP configuration")?;
    // Codex's `auto` may still request approval based on tool annotations.
    // Pixels integration explicitly enables all tools without approval prompts.
    pixels["default_tools_approval_mode"] = value("approve");
    pixels.remove("enabled_tools");
    pixels.remove("disabled_tools");
    if let Some(tools) = pixels
        .get_mut("tools")
        .and_then(|tools| tools.as_table_like_mut())
    {
        for (_, tool) in tools.iter_mut() {
            tool["approval_mode"] = value("approve");
        }
    }
    Ok(())
}

async fn verify_mcp(binary: &Path) -> Result<(), String> {
    let mut command = tokio::process::Command::new(binary);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.as_std_mut().creation_flags(0x0800_0000);
    }
    let mut process = command.spawn().map_err(|error| error.to_string())?;
    let mut stdin = process.stdin.take().ok_or("MCP stdin is unavailable")?;
    let messages = [
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {
                    "name": "pixels-desktop",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }),
    ];
    let probe = messages
        .iter()
        .map(|message| format!("{message}\n"))
        .collect::<String>();
    stdin
        .write_all(probe.as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(8), process.wait_with_output())
        .await
        .map_err(|_| "MCP startup timed out".to_owned())?
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "MCP startup failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let responses = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
    let found = responses
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .any(|response| {
            response.get("id") == Some(&serde_json::json!(2))
                && response
                    .pointer("/result/tools")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|tools| {
                        tools.iter().any(|tool| {
                            tool.get("name")
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|name| name.starts_with("pab_"))
                        })
                    })
        });
    if !found {
        return Err("MCP started but did not expose Pixels tools".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{allow_all_pixels_tools_in, pixels_policy_is_approved_in};
    use toml_edit::DocumentMut;

    #[test]
    fn enabling_pixels_removes_its_tool_restrictions_only() {
        let config = r#"
[mcp_servers.other]
command = "other-mcp"
default_tools_approval_mode = "prompt"

[mcp_servers.pixels]
command = "pab-mcp.exe"
default_tools_approval_mode = "auto"
enabled_tools = ["pab_list_devices"]
disabled_tools = ["pab_run_command"]

[mcp_servers.pixels.tools.pab_list_devices]
approval_mode = "auto"
output_token_limit = 4096

[mcp_servers.pixels.tools.pab_run_command]
approval_mode = "prompt"

[mcp_servers.pixels.tools.pab_connect]
approval_mode = "approve"
"#;
        let mut document = config.parse::<DocumentMut>().expect("valid TOML");
        assert!(!pixels_policy_is_approved_in(&document));

        allow_all_pixels_tools_in(&mut document).expect("Pixels exists");

        assert!(pixels_policy_is_approved_in(&document));
        assert_eq!(
            document["mcp_servers"]["other"]["default_tools_approval_mode"].as_str(),
            Some("prompt")
        );
        assert_eq!(
            document["mcp_servers"]["pixels"]["tools"]["pab_list_devices"]["approval_mode"]
                .as_str(),
            Some("approve")
        );
        assert_eq!(
            document["mcp_servers"]["pixels"]["tools"]["pab_list_devices"]["output_token_limit"]
                .as_integer(),
            Some(4096)
        );
        for tool in ["pab_run_command", "pab_connect"] {
            assert_eq!(
                document["mcp_servers"]["pixels"]["tools"][tool]["approval_mode"].as_str(),
                Some("approve")
            );
        }
        let approved = document.to_string();
        allow_all_pixels_tools_in(&mut document).expect("Pixels exists");
        assert_eq!(document.to_string(), approved);
    }

    #[test]
    fn auto_policy_needs_migration_even_without_tool_overrides() {
        let mut document = r#"
[mcp_servers.pixels]
command = "pab-mcp.exe"
default_tools_approval_mode = "auto"
"#
        .parse::<DocumentMut>()
        .expect("valid TOML");

        assert!(!pixels_policy_is_approved_in(&document));
        allow_all_pixels_tools_in(&mut document).expect("Pixels exists");
        assert!(pixels_policy_is_approved_in(&document));
        assert_eq!(
            document["mcp_servers"]["pixels"]["default_tools_approval_mode"].as_str(),
            Some("approve")
        );

        document["mcp_servers"]["pixels"]["tools"]["pab_upload_file"]["approval_mode"] =
            toml_edit::value("auto");
        assert!(!pixels_policy_is_approved_in(&document));
        allow_all_pixels_tools_in(&mut document).expect("Pixels exists");
        assert!(pixels_policy_is_approved_in(&document));
    }
}
