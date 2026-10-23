//! Per-user automatic registration. Never run this from elevated installers or services.
use super::*;
use pab_agent_core::{DataPaths, DataScope};

pub(super) fn settings_path() -> Result<PathBuf, String> {
    DataPaths::for_scope(DataScope::User)
        .map(|paths| paths.root().join("agent-integrations.toml"))
        .map_err(|error| error.to_string())
}

fn agent_key(id: AgentId) -> &'static str {
    match id {
        AgentId::Codex => "codex",
        AgentId::Kimi => "kimi",
        AgentId::Claude => "claude",
        AgentId::Deepseek => "deepseek",
        AgentId::Opencode => "opencode",
    }
}

fn allowed(document: &DocumentMut, id: AgentId) -> Result<bool, String> {
    let enabled = match document.get("auto_connect") {
        None => true,
        Some(item) => item.as_bool().ok_or("auto_connect must be a boolean")?,
    };
    let agent = match document.get("agents") {
        None => true,
        Some(item) => match item
            .as_table()
            .ok_or("agents must be a table")?
            .get(agent_key(id))
        {
            None => true,
            Some(item) => item.as_bool().ok_or("agent preferences must be booleans")?,
        },
    };
    Ok(enabled && agent)
}

pub(super) fn preference_edit(
    path: &Path,
    id: AgentId,
    enabled: bool,
) -> Result<config::Edit, String> {
    edit(path, |text| {
        let mut document = config::toml_document(text)?;
        // Validate before modifying even if the user is changing a different client.
        allowed(&document, id)?;
        if document.get("agents").is_none() {
            document["agents"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        document["agents"][agent_key(id)] = value(enabled);
        Ok(document.to_string())
    })
}

fn automatic_edits(
    state: &AgentIntegration,
    paths: &Location,
    mcp: &Path,
    preferences: &Path,
) -> Result<Vec<config::Edit>, String> {
    let text = config::read(preferences)?.unwrap_or_default();
    if !allowed(&config::toml_document(&text)?, state.id)?
        || !state.detected
        || !state.mcp_available
        || state.enabled
    {
        return Ok(Vec::new());
    }
    if let Some(error) = &state.error {
        return Err(error.clone());
    }
    // prepare rejects occupied names, invalid files and incompatible permissions.
    prepare(state.id, paths, mcp, true)
}

/// The ordinary Desktop runs in the signed-in user's context on all desktop OSes.
/// Every launch discovers newly installed clients and repairs our old install paths.
pub(crate) fn start() {
    tauri::async_runtime::spawn(async {
        // Serialize the whole sweep with UI reads/changes so they cannot see partial edits.
        let _guard = UPDATE_LOCK.lock().await;
        let preferences = match settings_path() {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(%error, "Could not read automatic AI integration preferences");
                return;
            }
        };
        let mcp = match mcp_binary() {
            Ok(path) if path.is_file() => path,
            _ => return, // Dev builds and incomplete packages must not rewrite host configs.
        };
        let mut verification: Option<Result<(), String>> = None;
        for id in AgentId::ALL {
            let result: Result<(), String> = async {
                let paths = location(id)?;
                let state = status(id).await;
                let edits = automatic_edits(&state, &paths, &mcp, &preferences)?;
                if edits.is_empty() {
                    return Ok(());
                }
                if verification.is_none() {
                    verification = Some(
                        tokio::time::timeout(Duration::from_secs(12), verify_mcp(&mcp))
                            .await
                            .map_err(|_| "MCP verification timed out".to_owned())
                            .and_then(|result| result),
                    );
                }
                verification.as_ref().unwrap().clone()?;
                config::commit(&edits)?;
                tracing::info!(?id, "Automatically configured Pixels AI integration");
                Ok(())
            }
            .await;
            if let Err(error) = result {
                tracing::warn!(?id, %error, "Automatic AI integration could not be completed");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_install_configures_all_detected_clients_and_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let mcp = root.path().join("pab-mcp.exe");
        fs::write(&mcp, "fixture").unwrap();
        let preferences = root.path().join("agent-integrations.toml");
        for id in AgentId::ALL {
            let paths = location_in(id, root.path(), None);
            let state = inspect(id, &paths, &mcp, true).unwrap();
            config::commit(&automatic_edits(&state, &paths, &mcp, &preferences).unwrap()).unwrap();
            let state = inspect(id, &paths, &mcp, true).unwrap();
            assert!(state.enabled, "{id:?}");
            assert!(
                automatic_edits(&state, &paths, &mcp, &preferences)
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(
            !preferences.exists(),
            "Defaults do not need a settings file"
        );
    }

    #[test]
    fn explicit_disable_survives_restart_and_manual_enable_restores_automatic_registration() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("agent-integrations.toml");
        fs::write(
            &path,
            "# Keep user settings\nauto_connect = true\n[agents]\nclaude = false\n",
        )
        .unwrap();
        config::commit(&[preference_edit(&path, AgentId::Codex, false).unwrap()]).unwrap();
        let doc = config::toml_document(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(!allowed(&doc, AgentId::Codex).unwrap());
        assert!(!allowed(&doc, AgentId::Claude).unwrap());
        assert!(allowed(&doc, AgentId::Kimi).unwrap());
        config::commit(&[preference_edit(&path, AgentId::Codex, true).unwrap()]).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# Keep user settings"));
        let doc = config::toml_document(&text).unwrap();
        assert!(allowed(&doc, AgentId::Codex).unwrap());
        assert!(!allowed(&doc, AgentId::Claude).unwrap());
        assert!(
            !allowed(
                &config::toml_document("auto_connect = false").unwrap(),
                AgentId::Codex
            )
            .unwrap()
        );
    }

    #[test]
    fn unavailable_opted_out_conflicting_and_invalid_clients_are_not_modified() {
        let root = tempfile::tempdir().unwrap();
        let mcp = root.path().join("pab-mcp.exe");
        fs::write(&mcp, "fixture").unwrap();
        let paths = location_in(AgentId::Codex, root.path(), None);
        let preferences = root.path().join("agent-integrations.toml");
        let mut state = inspect(AgentId::Codex, &paths, &mcp, false).unwrap();
        assert!(
            automatic_edits(&state, &paths, &mcp, &preferences)
                .unwrap()
                .is_empty()
        );
        state.detected = true;
        state.mcp_available = false;
        assert!(
            automatic_edits(&state, &paths, &mcp, &preferences)
                .unwrap()
                .is_empty()
        );
        state.mcp_available = true;
        config::commit(&[preference_edit(&preferences, AgentId::Codex, false).unwrap()]).unwrap();
        assert!(
            automatic_edits(&state, &paths, &mcp, &preferences)
                .unwrap()
                .is_empty()
        );
        fs::write(&preferences, "auto_connect = 'invalid'").unwrap();
        assert!(automatic_edits(&state, &paths, &mcp, &preferences).is_err());
        fs::write(&preferences, "").unwrap();
        fs::create_dir_all(paths.config.parent().unwrap()).unwrap();
        for text in [
            "invalid [toml",
            "[mcp_servers.pixels]\ncommand = 'another-mcp'\n",
        ] {
            fs::write(&paths.config, text).unwrap();
            assert!(automatic_edits(&state, &paths, &mcp, &preferences).is_err());
            assert_eq!(fs::read_to_string(&paths.config).unwrap(), text);
        }
    }

    #[test]
    fn upgrade_repairs_owned_mcp_path_and_preserves_other_servers() {
        let root = tempfile::tempdir().unwrap();
        let paths = location_in(AgentId::Codex, root.path(), None);
        let old = root.path().join("old/pab-mcp.exe");
        let new = root.path().join("new/pab-mcp.exe");
        fs::create_dir_all(new.parent().unwrap()).unwrap();
        fs::write(&new, "fixture").unwrap();
        config::commit(&prepare(AgentId::Codex, &paths, &old, true).unwrap()).unwrap();
        let mut text = fs::read_to_string(&paths.config).unwrap();
        text.push_str("\n[mcp_servers.other]\ncommand = 'keep-me'\n");
        fs::write(&paths.config, text).unwrap();
        let state = inspect(AgentId::Codex, &paths, &new, true).unwrap();
        config::commit(
            &automatic_edits(&state, &paths, &new, &root.path().join("prefs.toml")).unwrap(),
        )
        .unwrap();
        assert!(inspect(AgentId::Codex, &paths, &new, true).unwrap().enabled);
        assert!(
            fs::read_to_string(&paths.config)
                .unwrap()
                .contains("keep-me")
        );
    }
}
