//! Static MCP discovery preferences shared by Desktop and each MCP process.
use pab_agent_core::{DataPaths, DataScope, ensure_data_dir};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolGroup {
    Core,
    File,
    System,
    Desktop,
    Git,
    Container,
}
impl ToolGroup {
    pub const ALL: [Self; 6] = [
        Self::Core,
        Self::File,
        Self::System,
        Self::Desktop,
        Self::Git,
        Self::Container,
    ];
    pub fn tools(self) -> &'static [&'static str] {
        match self {
            Self::Core => &[
                "pab_list_devices",
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
            Self::System => &[
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
            version: 1,
            enabled_groups: ToolGroup::ALL.to_vec(),
        }
    }
}
impl McpToolSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
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
            .join("mcp-tools.json"))
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 4096 {
            return Err("MCP tool settings exceed 4096 bytes".into());
        }
        let settings: Self =
            serde_json::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes))
                .map_err(|e| e.to_string())?;
        settings.validate()?;
        Ok(settings)
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        let parent = path.parent().ok_or("MCP settings path has no parent")?;
        ensure_data_dir(parent).map_err(|e| e.to_string())?;
        let mut settings = self.clone();
        settings.enabled_groups.sort();
        // tempfile persists with atomic replacement on Windows and Unix: a new
        // MCP process sees either complete configuration, never partial JSON.
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_settings_roundtrip_atomic_replace_and_invalid_save_preserves_previous() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-tools.json");
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
            r#"{"version":2,"enabledGroups":["core"]}"#,
            r#"{"version":1,"enabledGroups":["core","core"]}"#,
            r#"{"version":1,"enabledGroups":["core","invalid"]}"#,
            r#"{"version":1,"enabledGroups":["core"],"other":true}"#,
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
    fn inventories_are_unique_and_core_controls_are_mandatory() {
        let names: Vec<_> = ToolGroup::ALL
            .into_iter()
            .flat_map(ToolGroup::tools)
            .copied()
            .collect();
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(names.len(), 64);
        assert_eq!(unique.len(), 64);
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
