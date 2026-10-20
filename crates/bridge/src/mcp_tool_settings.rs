//! Static MCP discovery preferences shared by Desktop and each MCP process.
use pab_agent_core::{DataPaths, DataScope, ensure_data_dir};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolGroup {
    Core,
    File,
    Document,
    System,
    Desktop,
    Git,
    Container,
    Browser,
    BrowserAdvanced,
}
impl ToolGroup {
    pub const ALL: [Self; 9] = [
        Self::Core,
        Self::File,
        Self::Document,
        Self::System,
        Self::Desktop,
        Self::Git,
        Self::Container,
        Self::Browser,
        Self::BrowserAdvanced,
    ];
    pub fn tools(self) -> &'static [&'static str] {
        match self {
            Self::Core => &[
                "pab_list_devices",
                "pab_list_execution_contexts",
                "pab_connect",
                "pab_run_command",
                "pab_get_task",
                "pab_read_output",
                "pab_open_terminal",
                "pab_terminal_input",
                "pab_terminal_read",
                "pab_terminal_resize",
                "pab_terminal_close",
                "pab_get_operation",
                "pab_cancel_operation",
                "pab_list_operations",
                "pab_disconnect",
            ],
            Self::File => &[
                "pab_download_file",
                "pab_file_hash",
                "pab_file_patch",
                "pab_file_read",
                "pab_file_search",
                "pab_file_stat",
                "pab_file_write",
                "pab_list_directory",
                "pab_mkdir",
                "pab_upload_file",
                "pab_file_copy",
                "pab_file_move",
                "pab_file_delete",
                "pab_archive_create",
                "pab_archive_extract",
            ],
            Self::Document => &[
                "pab_document_open",
                "pab_document_read",
                "pab_document_search",
                "pab_document_render_page",
                "pab_document_close",
            ],
            Self::System => &[
                "pab_port_forward_open",
                "pab_port_forward_status",
                "pab_port_forward_close",
                "pab_system_log_sources",
                "pab_system_log_read",
                "pab_system_log_event_read",
                "pab_get_process",
                "pab_get_service",
                "pab_list_disks",
                "pab_list_network_connections",
                "pab_list_network_interfaces",
                "pab_list_processes",
                "pab_list_services",
                "pab_list_sessions",
                "pab_resolve_dns",
                "pab_service_control",
                "pab_system_info",
                "pab_terminate_process",
            ],
            Self::Desktop => &[
                "pab_mobile_control",
                "pab_desktop_assistance",
                "pab_list_apps",
                "pab_launch_app",
                "pab_open_file",
                "pab_inspect_app_package",
                "pab_install_app",
                "pab_uninstall_app",
                "pab_get_app_change",
                "pab_resume_app_change",
                "pab_notification_status",
                "pab_list_notifications",
                "pab_wait_notification",
                "pab_open_notification",
                "pab_dismiss_notification",
                "pab_capture_screenshot",
                "pab_desktop_input",
                "pab_focus_window",
                "pab_list_monitors",
                "pab_list_windows",
                "pab_type_text",
                "pab_ui_query",
                "pab_ui_get",
                "pab_ui_action",
                "pab_ui_wait",
                "pab_window_control",
            ],
            Self::Git => &[
                "pab_git_status",
                "pab_git_diff",
                "pab_git_log",
                "pab_git_commit",
                "pab_git_checkout",
                "pab_git_fetch",
                "pab_git_pull",
                "pab_git_push",
            ],
            Self::Container => &[
                "pab_container_control",
                "pab_container_logs",
                "pab_get_container",
                "pab_list_containers",
            ],
            Self::Browser => &[
                "pab_browser_setup_info",
                "pab_browser_attach",
                "pab_browser_get_session",
                "pab_browser_close",
                "pab_browser_open",
                "pab_browser_navigate",
                "pab_browser_tabs",
                "pab_browser_snapshot",
                "pab_browser_find",
                "pab_browser_extract",
                "pab_browser_screenshot",
                "pab_browser_wait",
                "pab_browser_click",
                "pab_browser_fill",
                "pab_browser_fill_form",
                "pab_browser_press_key",
                "pab_browser_select_option",
                "pab_browser_set_checked",
                "pab_browser_hover",
                "pab_browser_scroll",
                "pab_browser_drag",
                "pab_browser_handle_dialog",
                "pab_browser_file_upload",
                "pab_browser_downloads",
                "pab_browser_request_handoff",
                "pab_browser_resume",
            ],
            Self::BrowserAdvanced => &[
                "pab_browser_pointer",
                "pab_browser_drop",
                "pab_browser_console_messages",
                "pab_browser_network_requests",
                "pab_browser_pdf_save",
            ],
        }
    }
    pub fn for_tool(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|group| group.tools().contains(&name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpToolSettings {
    pub version: u32,
    pub enabled_groups: Vec<ToolGroup>,
}
impl Default for McpToolSettings {
    fn default() -> Self {
        Self {
            version: 3,
            enabled_groups: ToolGroup::ALL.to_vec(),
        }
    }
}
impl McpToolSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 3 {
            return Err("Unsupported MCP tool settings version".into());
        }
        if !self.enabled_groups.contains(&ToolGroup::Core) {
            return Err(
                "The core tool group is required for connections and existing operations".into(),
            );
        }
        let mut groups = self.enabled_groups.clone();
        groups.sort();
        groups.dedup();
        if groups.len() != self.enabled_groups.len() {
            return Err("Duplicate MCP tool groups".into());
        }
        Ok(())
    }
    pub fn allows(&self, name: &str) -> bool {
        ToolGroup::for_tool(name).is_some_and(|group| self.enabled_groups.contains(&group))
    }
    pub fn path() -> Result<PathBuf, String> {
        Ok(DataPaths::for_scope(DataScope::User)
            .map_err(|e| e.to_string())?
            .root()
            .join("mcp-tools.toml"))
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        Ok(pab_service_config::preferences::load_with_limit(
            path,
            |settings: &mut Self| {
                if settings.version == 1 {
                    // Version 1 predates both browser groups. Enable both on upgrade
                    // while preserving the user's choices for existing groups.
                    for group in [ToolGroup::Browser, ToolGroup::BrowserAdvanced] {
                        if !settings.enabled_groups.contains(&group) {
                            settings.enabled_groups.push(group);
                        }
                    }
                    settings.version = 2;
                }
                if settings.version == 2 {
                    // Newly introduced document tools are enabled for existing users.
                    // Preserve the opt-out state of every earlier group.
                    settings.enabled_groups.push(ToolGroup::Document);
                    settings.version = 3;
                }
                settings.validate()?;
                Ok(())
            },
            4096,
        )?
        .unwrap_or_default())
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        let parent = path.parent().ok_or("MCP settings path has no parent")?;
        ensure_data_dir(parent).map_err(|e| e.to_string())?;
        let mut settings = self.clone();
        settings.enabled_groups.sort();
        pab_service_config::preferences::save(path, &settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn strict_settings_roundtrip_atomic_replace_and_invalid_save_preserves_previous() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-tools.toml");
        assert_eq!(
            McpToolSettings::load(&path).unwrap(),
            McpToolSettings::default()
        );
        let mut settings = McpToolSettings::default();
        settings.save(&path).unwrap();
        settings.enabled_groups = vec![ToolGroup::Core, ToolGroup::File];
        settings.save(&path).unwrap();
        assert_eq!(McpToolSettings::load(&path).unwrap(), settings);
        assert!(settings.allows("pab_get_operation"));
        assert!(!settings.allows("pab_git_push"));
        assert!(!settings.allows("unknown"));
        settings.enabled_groups = vec![ToolGroup::Git];
        assert!(settings.save(&path).is_err());
        assert_eq!(
            McpToolSettings::load(&path).unwrap().enabled_groups,
            vec![ToolGroup::Core, ToolGroup::File]
        );
        for data in [
            "version = 4\nenabledGroups = ['core']",
            "version = 3\nenabledGroups = ['core', 'core']",
            "version = 3\nenabledGroups = ['core', 'invalid']",
            "version = 3\nenabledGroups = ['core']\nother = true",
            "{",
            "",
        ] {
            fs::write(&path, data).unwrap();
            assert!(McpToolSettings::load(&path).is_err(), "{data}");
        }
        fs::write(&path, vec![b' '; 4097]).unwrap();
        assert!(McpToolSettings::load(&path).is_err());
    }
    #[test]
    fn older_selections_gain_document_tools_and_keep_browser_opt_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-tools.toml");
        fs::write(
            path.with_extension("json"),
            r#"{"version":1,"enabledGroups":["core","file"]}"#,
        )
        .unwrap();
        let migrated = McpToolSettings::load(&path).unwrap();
        assert_eq!(migrated.version, 3);
        assert!(migrated.enabled_groups.contains(&ToolGroup::Browser));
        assert!(
            migrated
                .enabled_groups
                .contains(&ToolGroup::BrowserAdvanced)
        );
        assert!(migrated.enabled_groups.contains(&ToolGroup::Document));
        assert!(!migrated.enabled_groups.contains(&ToolGroup::Git));
        let mut disabled = migrated;
        disabled
            .enabled_groups
            .retain(|group| *group != ToolGroup::Browser);
        disabled.save(&path).unwrap();
        assert!(
            !McpToolSettings::load(&path)
                .unwrap()
                .allows("pab_browser_attach")
        );
        assert!(McpToolSettings::default().allows("pab_browser_pointer"));
        assert!(McpToolSettings::default().allows("pab_browser_pdf_save"));
        let mut v2_opt_out = McpToolSettings::default();
        v2_opt_out
            .enabled_groups
            .retain(|group| *group != ToolGroup::BrowserAdvanced);
        v2_opt_out.save(&path).unwrap();
        assert!(
            !McpToolSettings::load(&path)
                .unwrap()
                .allows("pab_browser_pointer")
        );
        fs::remove_file(&path).unwrap();
        fs::write(
            path.with_extension("json"),
            r#"{"version":2,"enabledGroups":["core","file"]}"#,
        )
        .unwrap();
        let migrated_v2 = McpToolSettings::load(&path).unwrap();
        assert_eq!(migrated_v2.version, 3);
        assert!(migrated_v2.allows("pab_document_open"));
        assert!(!migrated_v2.allows("pab_browser_attach"));
    }
    #[test]
    fn inventories_are_unique_and_core_controls_are_mandatory() {
        let names: Vec<_> = ToolGroup::ALL
            .into_iter()
            .flat_map(ToolGroup::tools)
            .copied()
            .collect();
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert!(!names.is_empty());
        assert_eq!(unique.len(), names.len());
        for name in [
            "pab_connect",
            "pab_disconnect",
            "pab_get_operation",
            "pab_cancel_operation",
            "pab_list_operations",
            "pab_get_task",
            "pab_read_output",
        ] {
            assert_eq!(ToolGroup::for_tool(name), Some(ToolGroup::Core));
        }
    }
}
