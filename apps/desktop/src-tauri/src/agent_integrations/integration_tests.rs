use super::*;

#[test]
fn dsh_npm_and_npx_installations_do_not_require_a_path_launcher() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("user npm prefix");
    let cache = root.path().join("npm-cache");
    let global = prefix.join("node_modules/@deepseek-ai/dsh");
    let npx = cache.join("_npx/fixture/node_modules/@deepseek-ai/dsh");
    let manifest = r#"{"name":"@deepseek-ai/dsh","bin":{"dsh":"lib/dsh-entry.js"}}"#;
    write(&npx.join("package.json"), manifest);
    assert!(discovery::find_agent(AgentId::Deepseek, &[], &[cache.clone()]).is_none());
    write(&npx.join("lib/dsh-entry.js"), "fixture");
    assert_eq!(
        discovery::find_agent(AgentId::Deepseek, &[], &[cache.clone()]),
        Some(npx.join("lib/dsh-entry.js"))
    );
    assert!(discovery::find_agent(AgentId::Claude, &[], &[cache]).is_none());
    write(&global.join("package.json"), manifest);
    write(&global.join("lib/dsh-entry.js"), "fixture");
    assert_eq!(
        discovery::find_agent(AgentId::Deepseek, &[prefix.clone()], &[]),
        Some(global.join("lib/dsh-entry.js"))
    );
    write(
        &global.join("package.json"),
        r#"{"name":"other","bin":{"dsh":"lib/dsh-entry.js"}}"#,
    );
    assert!(discovery::find_agent(AgentId::Deepseek, &[prefix], &[]).is_none());
}

#[test]
fn client_detection_refresh_observes_newly_installed_launcher() {
    let root = tempfile::tempdir().unwrap();
    let dirs = vec![root.path().join("AppData/Roaming/npm")];
    assert!(discovery::find_agent(AgentId::Deepseek, &dirs, &[]).is_none());
    let launcher = dirs[0].join(if cfg!(windows) { "dsh.cmd" } else { "dsh" });
    write(&launcher, "fixture");
    assert_eq!(
        discovery::find_agent(AgentId::Deepseek, &dirs, &[]),
        Some(launcher.clone())
    );
    fs::remove_file(launcher).unwrap();
    assert!(discovery::find_agent(AgentId::Deepseek, &dirs, &[]).is_none());
}

fn fixture(id: AgentId) -> (tempfile::TempDir, Location, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join(if cfg!(windows) {
        "目录 with spaces/pab-mcp.exe"
    } else {
        "目录 with spaces/run-mcp.sh"
    });
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"fixture").unwrap();
    let location = location_in(id, root.path(), None);
    (root, location, binary)
}

#[test]
fn jsonc_clients_preserve_comments_and_other_services() {
    for (id, field) in [(AgentId::Opencode, "mcp")] {
        let (_root, paths, binary) = fixture(id);
        let original = format!(
            "{{\n  // Keep this model comment\n  \"model\": \"fixture/model\",\n  \"{field}\": {{\n    // Other server comment\n    \"other\": {{\"command\": [\"other\"],}},\n  }},\n}}"
        );
        write(&paths.config, &original);
        config::commit(&prepare(id, &paths, &binary, true).unwrap()).unwrap();
        assert!(inspect(id, &paths, &binary, true).unwrap().enabled);
        config::commit(&prepare(id, &paths, &binary, false).unwrap()).unwrap();
        let text = fs::read_to_string(&paths.config).unwrap();
        assert!(text.contains("// Keep this model comment"));
        assert!(text.contains("// Other server comment"));
        let doc: Value = jsonc_parser::parse_to_serde_value(&text, &Default::default()).unwrap();
        assert_eq!(doc["model"], "fixture/model");
        assert_eq!(doc[field]["other"]["command"], json!(["other"]));
    }
}

#[test]
fn opencode_permission_order_and_scalar_restore() {
    for permission in [json!("ask"), json!("deny"), json!("allow")] {
        let (_root, paths, binary) = fixture(AgentId::Opencode);
        write(&paths.config, &json!({"permission":permission}).to_string());
        config::commit(&prepare(AgentId::Opencode, &paths, &binary, true).unwrap()).unwrap();
        assert!(
            inspect(AgentId::Opencode, &paths, &binary, true)
                .unwrap()
                .enabled
        );
        config::commit(&prepare(AgentId::Opencode, &paths, &binary, false).unwrap()).unwrap();
        let doc: Value = jsonc_parser::parse_to_serde_value(
            &fs::read_to_string(&paths.config).unwrap(),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(doc["permission"], permission);
    }
    let (_root, paths, binary) = fixture(AgentId::Opencode);
    write(
        &paths.config,
        r#"{"permission":{"pixels_*":"ask","*":"deny","read":"allow"}}"#,
    );
    config::commit(&prepare(AgentId::Opencode, &paths, &binary, true).unwrap()).unwrap();
    let doc = jsonc_parser::cst::CstRootNode::parse(
        &fs::read_to_string(&paths.config).unwrap(),
        &Default::default(),
    )
    .unwrap();
    let keys = doc
        .object_value()
        .unwrap()
        .object_value("permission")
        .unwrap()
        .properties()
        .iter()
        .map(|p| p.decoded_name().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(keys, ["*", "read", "pixels_*"]);
    config::commit(&prepare(AgentId::Opencode, &paths, &binary, false).unwrap()).unwrap();
    let text = fs::read_to_string(&paths.config).unwrap();
    let doc = jsonc_parser::cst::CstRootNode::parse(&text, &Default::default()).unwrap();
    assert_eq!(
        doc.object_value()
            .unwrap()
            .object_value("permission")
            .unwrap()
            .properties()
            .first()
            .unwrap()
            .decoded_name()
            .as_deref(),
        Some("pixels_*")
    );
    let data: Value = jsonc_parser::parse_to_serde_value(&text, &Default::default()).unwrap();
    assert_eq!(
        data["permission"],
        json!({"pixels_*":"ask","*":"deny","read":"allow"})
    );
}

#[test]
fn opencode_rejects_shadow_entries_and_filters() {
    let (_root, paths, binary) = fixture(AgentId::Opencode);
    write(&paths.config, r#"{"tools":{"pixels_*":false}}"#);
    assert!(prepare(AgentId::Opencode, &paths, &binary, true).is_err());
    write(&paths.config, "{}");
    write(
        &paths.config.with_file_name("opencode.json"),
        r#"{"mcp":{"pixels":{"command":["other"]}}}"#,
    );
    assert!(prepare(AgentId::Opencode, &paths, &binary, true).is_err());
}

#[test]
fn duplicate_jsonc_keys_are_never_overwritten() {
    for id in [AgentId::Opencode] {
        let (_root, paths, binary) = fixture(id);
        write(&paths.config, r#"{"mcp":{},"mcp":{}}"#);
        assert!(prepare(id, &paths, &binary, true).is_err());
    }
}

#[test]
fn opencode_npm_is_detected_without_path() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("bin");
    let package = dir.join("node_modules/opencode-ai");
    write(
        &package.join("package.json"),
        r#"{"name":"opencode-ai","bin":{"opencode":"bin/opencode"}}"#,
    );
    write(&package.join("bin/opencode"), "fixture");
    assert_eq!(
        discovery::find_agent(AgentId::Opencode, &[dir], &[]),
        Some(package.join("bin/opencode"))
    );
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn all_clients_enable_read_only_status_repeat_disable() {
    for id in AgentId::ALL {
        let (_root, paths, binary) = fixture(id);
        let empty = inspect(id, &paths, &binary, true).unwrap();
        assert!(!empty.enabled);
        assert!(!paths.config.exists());
        config::commit(&prepare(id, &paths, &binary, true).unwrap()).unwrap();
        assert!(
            inspect(id, &paths, &binary, true).unwrap().enabled,
            "{id:?}"
        );
        let configured = fs::read(&paths.config).unwrap();
        inspect(id, &paths, &binary, true).unwrap();
        assert_eq!(fs::read(&paths.config).unwrap(), configured);
        config::commit(&prepare(id, &paths, &binary, true).unwrap()).unwrap();
        assert_eq!(
            fs::read(&paths.config).unwrap(),
            configured,
            "idempotence {id:?}"
        );
        config::commit(&prepare(id, &paths, &binary, false).unwrap()).unwrap();
        assert!(!inspect(id, &paths, &binary, true).unwrap().configured);
        config::commit(&prepare(id, &paths, &binary, false).unwrap()).unwrap();
    }
}

#[test]
fn codex_repairs_permissions_and_path_but_read_never_writes() {
    let (_root, paths, binary) = fixture(AgentId::Codex);
    let old = "# user comment\n[mcp_servers.other]\ncommand='other'\n[mcp_servers.pixels]\ncommand='old/pab-mcp.exe'\ndefault_tools_approval_mode='auto'\n";
    write(&paths.config, old);
    assert!(
        !inspect(AgentId::Codex, &paths, &binary, true)
            .unwrap()
            .enabled
    );
    assert_eq!(fs::read_to_string(&paths.config).unwrap(), old);
    config::commit(&prepare(AgentId::Codex, &paths, &binary, true).unwrap()).unwrap();
    let updated = fs::read_to_string(&paths.config).unwrap();
    assert!(updated.contains("# user comment"));
    assert!(updated.contains("command='other'"));
    assert!(
        inspect(AgentId::Codex, &paths, &binary, true)
            .unwrap()
            .enabled
    );
}

#[test]
fn malformed_and_third_party_entries_never_get_overwritten() {
    for id in AgentId::ALL {
        let (_root, paths, binary) = fixture(id);
        write(&paths.config, "{ malformed [ config");
        assert!(prepare(id, &paths, &binary, true).is_err());
        assert_eq!(
            fs::read_to_string(&paths.config).unwrap(),
            "{ malformed [ config"
        );
        let third_party = match id {
            AgentId::Codex => "[mcp_servers.pixels]\ncommand='third-party'\n",
            AgentId::Kimi | AgentId::Claude => {
                r#"{"mcpServers":{"pixels":{"command":"third-party"}}}"#
            }
            AgentId::Deepseek => {
                "- insert:\n  - id: custom\n    name: other\n    config:\n      serverName: pixels\n      command: third-party\n"
            }
            AgentId::Opencode => r#"{"mcp":{"pixels":{"type":"local","command":["third-party"]}}}"#,
        };
        write(&paths.config, third_party);
        assert!(inspect(id, &paths, &binary, true).unwrap().conflict);
        assert!(prepare(id, &paths, &binary, true).is_err());
        assert!(prepare(id, &paths, &binary, false).is_err());
        assert_eq!(fs::read_to_string(&paths.config).unwrap(), third_party);
    }
}

#[test]
fn json_merge_preserves_other_servers_and_claude_account_data() {
    for id in [AgentId::Kimi, AgentId::Claude] {
        let (_root, paths, binary) = fixture(id);
        write(
            &paths.config,
            r#"{"account":{"fixture":"keep"},"mcpServers":{"other":{"command":"other","env":{"TOKEN":"fixture"}}}}"#,
        );
        config::commit(&prepare(id, &paths, &binary, true).unwrap()).unwrap();
        config::commit(&prepare(id, &paths, &binary, false).unwrap()).unwrap();
        let data = config::json_document(&fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert_eq!(data["account"]["fixture"], "keep");
        assert_eq!(data["mcpServers"]["other"]["env"]["TOKEN"], "fixture");
    }
}

#[test]
fn kimi_allow_precedes_asks_and_disable_restores_user_rules() {
    let mut doc = config::toml_document("# user\n[[permission.rules]]\ndecision='ask'\npattern='mcp__*'\n[[permission.rules]]\ndecision='allow'\npattern='mcp__pixels__*'\n").unwrap();
    config::set_kimi_permission(&mut doc, true).unwrap();
    assert!(config::kimi_approved(&doc));
    let once = doc.to_string();
    config::set_kimi_permission(&mut doc, true).unwrap();
    assert_eq!(once, doc.to_string());
    config::set_kimi_permission(&mut doc, false).unwrap();
    let rules = doc["permission"]["rules"].as_array_of_tables().unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules.get(0).unwrap()["decision"].as_str(), Some("ask"));
    assert_eq!(rules.get(1).unwrap()["decision"].as_str(), Some("allow"));
}

#[test]
fn claude_existing_grants_survive_and_owned_grants_are_removed() {
    for existing in [false, true] {
        let (_root, paths, binary) = fixture(AgentId::Claude);
        let mut document = json!({"permissions":{"allow":["Read"]}});
        if existing {
            document["permissions"]["allow"]
                .as_array_mut()
                .unwrap()
                .push(json!(config::PERMISSION));
        }
        write(&paths.permissions, &config::json_text(&document));
        config::commit(&prepare(AgentId::Claude, &paths, &binary, true).unwrap()).unwrap();
        config::commit(&prepare(AgentId::Claude, &paths, &binary, true).unwrap()).unwrap();
        config::commit(&prepare(AgentId::Claude, &paths, &binary, false).unwrap()).unwrap();
        let after =
            config::json_document(&fs::read_to_string(&paths.permissions).unwrap()).unwrap();
        assert_eq!(after, document);
    }
}

#[test]
fn claude_conflicting_deny_or_ask_does_not_claim_auto_approval() {
    for rule in [
        "mcp__pixels__pab_run_command",
        "mcp__*",
        "*",
        "mcp__pix*__*",
    ] {
        for kind in ["ask", "deny"] {
            let (_root, paths, binary) = fixture(AgentId::Claude);
            write(
                &paths.permissions,
                &json!({"permissions":{kind:[rule]}}).to_string(),
            );
            assert!(
                prepare(AgentId::Claude, &paths, &binary, true).is_err(),
                "{kind}: {rule}"
            );
            assert!(!paths.config.exists());
        }
    }
}

#[test]
fn dsh_keeps_user_yaml_comments_and_javascript_tags_verbatim() {
    let (_root, paths, binary) = fixture(AgentId::Deepseek);
    let original =
        "# user's overlay\n- id: other\n  config:\n    token: !!js process.env.EXAMPLE_TOKEN\n";
    write(&paths.config, original);
    config::commit(&prepare(AgentId::Deepseek, &paths, &binary, true).unwrap()).unwrap();
    let after = fs::read_to_string(&paths.config).unwrap();
    assert!(after.starts_with(original));
    assert!(after.contains("toolCallTimeoutMs: 180000"));
    config::commit(&prepare(AgentId::Deepseek, &paths, &binary, false).unwrap()).unwrap();
    assert_eq!(fs::read_to_string(&paths.config).unwrap(), original);
}

#[test]
fn dsh_rejects_duplicate_unmanaged_entries_and_broken_markers() {
    let binary = Path::new("/app/run-mcp.sh");
    assert!(config::set_dsh("# BEGIN PIXELS AGENT BRIDGE\n[]\n", binary, true).is_err());
    assert!(config::set_dsh("key: value\n", binary, true).is_err());
    assert!(
        config::set_dsh(
            "- insert:\n  - config: {serverName: pixels, command: '/app/run-mcp.sh'}\n",
            binary,
            true
        )
        .is_err()
    );
}

#[test]
fn snapshots_prevent_overwriting_edits_made_after_prepare() {
    let (_root, paths, binary) = fixture(AgentId::Claude);
    let edits = prepare(AgentId::Claude, &paths, &binary, true).unwrap();
    write(&paths.config, "{\"newUserChange\":true}");
    assert!(config::commit(&edits).is_err());
    assert!(!paths.permissions.exists());
    assert!(!paths.receipt.exists());
    assert_eq!(
        fs::read_to_string(&paths.config).unwrap(),
        "{\"newUserChange\":true}"
    );
}

#[test]
fn invalid_write_location_does_not_leave_partial_configuration() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("settings.json");
    let blocked_parent = root.path().join("not-a-directory");
    write(&first, "original");
    write(&blocked_parent, "file");
    let result = config::commit(&[
        config::Edit {
            path: first.clone(),
            before: Some("original".into()),
            after: "changed".into(),
        },
        config::Edit {
            path: blocked_parent.join("config.json"),
            before: None,
            after: "{}".into(),
        },
    ]);
    assert!(result.is_err());
    assert_eq!(fs::read_to_string(first).unwrap(), "original");
}

#[test]
fn dsh_crlf_status_and_external_override_conflict() {
    let (_root, paths, binary) = fixture(AgentId::Deepseek);
    let original = config::set_dsh("", &binary, true)
        .unwrap()
        .replace('\n', "\r\n");
    write(&paths.config, &original);
    assert!(
        inspect(AgentId::Deepseek, &paths, &binary, true)
            .unwrap()
            .enabled
    );
    write(
        &paths.config,
        &format!("{original}\n- id: pixels-agent-bridge-mcp\n  disabled: true\n"),
    );
    assert!(prepare(AgentId::Deepseek, &paths, &binary, true).is_err());
}

#[cfg(unix)]
#[test]
fn config_symlink_is_never_replaced() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("shared.json");
    let link = root.path().join("settings.json");
    let first = root.path().join("mcp.json");
    write(&first, "before");
    write(&target, "original");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(
        config::commit(&[
            config::Edit {
                path: first.clone(),
                before: Some("before".into()),
                after: "after".into(),
            },
            config::Edit {
                path: link.clone(),
                before: Some("original".into()),
                after: "changed".into()
            }
        ])
        .is_err()
    );
    assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_to_string(target).unwrap(), "original");
    assert_eq!(
        fs::read_to_string(first).unwrap(),
        "before",
        "earlier write must be rolled back"
    );
}

#[test]
fn custom_roots_are_used_for_every_client() {
    let home = Path::new("/home/example");
    for id in AgentId::ALL {
        let paths = location_in(id, home, Some(PathBuf::from("/custom home/用户")));
        assert!(paths.config.starts_with("/custom home/用户"));
        assert!(paths.permissions.starts_with("/custom home/用户"));
    }
    assert_eq!(
        location_in(AgentId::Claude, home, None).config,
        home.join(".claude.json")
    );
    assert_eq!(
        location_in(AgentId::Kimi, home, None).config,
        home.join(".kimi-code/mcp.json")
    );
}

#[tokio::test]
#[ignore = "requires PAB_INTEGRATION_FIXTURES and PAB_INTEGRATION_MCP for isolated client acceptance"]
async fn export_and_probe_real_client_fixtures() {
    let root = PathBuf::from(env::var_os("PAB_INTEGRATION_FIXTURES").unwrap());
    let binary = PathBuf::from(env::var_os("PAB_INTEGRATION_MCP").unwrap());
    assert!(root.is_absolute() && binary.is_file());
    verify_mcp(&binary).await.unwrap();
    for id in AgentId::ALL {
        let home = root.join(id.executable());
        let paths = location_in(id, &home, Some(home.clone()));
        config::commit(&prepare(id, &paths, &binary, true).unwrap()).unwrap();
        assert!(inspect(id, &paths, &binary, true).unwrap().enabled);
    }
}
