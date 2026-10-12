//! Browser operations are intentionally a closed vocabulary. Web content is
//! untrusted, and callers cannot select an upstream Playwright tool directly.
use crate::{OperatorRef, RequestId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const BROWSER_HELPER_SCHEMA_VERSION: u16 = 1;
pub const MAX_BROWSER_ARGUMENT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAction {
    SetupInfo,
    Attach,
    GetSession,
    Close,
    Open,
    Navigate,
    Tabs,
    Snapshot,
    Find,
    Extract,
    Screenshot,
    Wait,
    Click,
    Fill,
    FillForm,
    PressKey,
    SelectOption,
    SetChecked,
    Hover,
    Scroll,
    Drag,
    HandleDialog,
    FileUpload,
    Downloads,
    RequestHandoff,
    Resume,
    Pointer,
    Drop,
    ConsoleMessages,
    NetworkRequests,
    PdfSave,
}

impl BrowserAction {
    pub const ALL: [Self; 31] = [
        Self::SetupInfo,
        Self::Attach,
        Self::GetSession,
        Self::Close,
        Self::Open,
        Self::Navigate,
        Self::Tabs,
        Self::Snapshot,
        Self::Find,
        Self::Extract,
        Self::Screenshot,
        Self::Wait,
        Self::Click,
        Self::Fill,
        Self::FillForm,
        Self::PressKey,
        Self::SelectOption,
        Self::SetChecked,
        Self::Hover,
        Self::Scroll,
        Self::Drag,
        Self::HandleDialog,
        Self::FileUpload,
        Self::Downloads,
        Self::RequestHandoff,
        Self::Resume,
        Self::Pointer,
        Self::Drop,
        Self::ConsoleMessages,
        Self::NetworkRequests,
        Self::PdfSave,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::SetupInfo => "browser_setup_info",
            Self::Attach => "browser_attach",
            Self::GetSession => "browser_get_session",
            Self::Close => "browser_close",
            Self::Open => "browser_open",
            Self::Navigate => "browser_navigate",
            Self::Tabs => "browser_tabs",
            Self::Snapshot => "browser_snapshot",
            Self::Find => "browser_find",
            Self::Extract => "browser_extract",
            Self::Screenshot => "browser_screenshot",
            Self::Wait => "browser_wait",
            Self::Click => "browser_click",
            Self::Fill => "browser_fill",
            Self::FillForm => "browser_fill_form",
            Self::PressKey => "browser_press_key",
            Self::SelectOption => "browser_select_option",
            Self::SetChecked => "browser_set_checked",
            Self::Hover => "browser_hover",
            Self::Scroll => "browser_scroll",
            Self::Drag => "browser_drag",
            Self::HandleDialog => "browser_handle_dialog",
            Self::FileUpload => "browser_file_upload",
            Self::Downloads => "browser_downloads",
            Self::RequestHandoff => "browser_request_handoff",
            Self::Resume => "browser_resume",
            Self::Pointer => "browser_pointer",
            Self::Drop => "browser_drop",
            Self::ConsoleMessages => "browser_console_messages",
            Self::NetworkRequests => "browser_network_requests",
            Self::PdfSave => "browser_pdf_save",
        }
    }

    pub const fn is_mutation(self) -> bool {
        !matches!(
            self,
            Self::SetupInfo
                | Self::GetSession
                | Self::Snapshot
                | Self::Find
                | Self::Extract
                | Self::Screenshot
                | Self::Wait
                | Self::Downloads
                | Self::ConsoleMessages
                | Self::NetworkRequests
                | Self::PdfSave
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserRequest {
    pub action: BrowserAction,
    pub session_ref: Option<RequestId>,
    pub arguments: Value,
    /// Executor supplies this from the authenticated operator. MCP input may
    /// never choose or override the owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<OperatorRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixels_user: Option<crate::UserId>,
}

impl BrowserRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        let Some(args) = self.arguments.as_object() else {
            return Err("browser arguments must be an object");
        };
        if serde_json::to_vec(&self.arguments)
            .map_err(|_| "invalid browser arguments")?
            .len()
            > MAX_BROWSER_ARGUMENT_BYTES
        {
            return Err("browser arguments must be an object up to 16 KiB");
        }
        if matches!(
            self.action,
            BrowserAction::Attach | BrowserAction::SetupInfo
        ) && self.session_ref.is_some()
        {
            return Err("browser setup cannot select an existing session");
        }
        if !matches!(
            self.action,
            BrowserAction::Attach | BrowserAction::SetupInfo
        ) && self.session_ref.is_none()
        {
            return Err("browser session_ref is required");
        }
        use BrowserAction::*;
        let allowed: &[&str] = match self.action {
            SetupInfo => &[],
            Attach => &["mode"],
            GetSession | Close | Snapshot | Screenshot | Resume | PdfSave => &[],
            Downloads => &["filename"],
            Open => &["url"],
            Navigate => &["url", "direction"],
            Tabs => &["action", "index", "url"],
            Find => &["text", "regex"],
            Extract => &["target", "kind", "limit"],
            Wait => &["text", "text_gone", "time"],
            Click => &["target", "element", "double_click", "button"],
            Fill => &["target", "element", "text", "slowly", "submit"],
            FillForm => &["fields"],
            PressKey => &["key"],
            SelectOption => &["target", "element", "values"],
            SetChecked => &["target", "checked"],
            Hover => &["target", "element"],
            Scroll => &["direction", "pages"],
            Drag => &["start_target", "end_target"],
            HandleDialog => &["accept", "prompt_text"],
            FileUpload => &["paths"],
            RequestHandoff => &["reason"],
            Pointer => &["action", "x", "y", "delta_x", "delta_y", "button"],
            Drop => &["target", "paths", "data"],
            ConsoleMessages => &["level", "all"],
            NetworkRequests => &["static", "filter"],
        };
        if args.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err("unsupported browser argument");
        }
        let text = |key: &str, max: usize| -> Result<Option<&str>, &'static str> {
            match args.get(key) {
                None => Ok(None),
                Some(Value::String(value)) if !value.is_empty() && value.len() <= max => {
                    Ok(Some(value))
                }
                _ => Err("invalid browser text argument"),
            }
        };
        let required = |key: &str, max: usize| -> Result<&str, &'static str> {
            text(key, max)?.ok_or("missing browser argument")
        };
        let boolean = |key: &str| -> Result<(), &'static str> {
            if args.get(key).is_some_and(|value| !value.is_boolean()) {
                Err("invalid browser boolean argument")
            } else {
                Ok(())
            }
        };
        match self.action {
            SetupInfo => {}
            Attach => {
                let mode = text("mode", 32)?.unwrap_or("pixels");
                if mode != "pixels" {
                    return Err("invalid browser mode");
                }
            }
            Open => {
                http_url(required("url", 2048)?)?;
            }
            Navigate => {
                let url = text("url", 2048)?;
                let direction = text("direction", 16)?;
                if url.is_some() == direction.is_some() {
                    return Err("provide exactly url or direction");
                }
                if direction.is_some_and(|value| !["back", "forward", "reload"].contains(&value)) {
                    return Err("invalid navigation direction");
                }
                if let Some(url) = url {
                    http_url(url)?;
                }
            }
            Tabs => {
                let action = required("action", 16)?;
                if !["list", "new", "select"].contains(&action) {
                    return Err("unsupported tab action");
                }
                if let Some(index) = args.get("index") {
                    if index.as_u64().is_none_or(|value| value > 1000) {
                        return Err("invalid tab index");
                    }
                }
                if action == "select" && !args.contains_key("index") {
                    return Err("tab index required");
                }
                if action != "select" && args.contains_key("index") {
                    return Err("tab index not applicable");
                }
                if action == "new" {
                    http_url(required("url", 2048)?)?;
                } else if args.contains_key("url") {
                    return Err("tab URL not applicable");
                }
            }
            Find => {
                if text("text", 512)?.is_some() == text("regex", 512)?.is_some() {
                    return Err("provide exactly text or regex");
                }
            }
            Extract => {
                required("target", 256)?;
                if !["text", "link", "attributes", "table", "value"]
                    .contains(&required("kind", 16)?)
                {
                    return Err("invalid extraction kind");
                }
                if args
                    .get("limit")
                    .is_some_and(|value| value.as_u64().is_none_or(|n| n == 0 || n > 100))
                {
                    return Err("invalid extraction limit");
                }
            }
            Wait => {
                let has_text = text("text", 512)?.is_some() || text("text_gone", 512)?.is_some();
                let duration = args
                    .get("time")
                    .map(|value| value.as_f64().ok_or("invalid wait duration"))
                    .transpose()?;
                if duration.is_some_and(|value| !(0.0..=30.0).contains(&value)) {
                    return Err("invalid wait duration");
                }
                if !has_text && duration.is_none() {
                    return Err("wait condition required");
                }
            }
            Click => {
                required("target", 256)?;
                text("element", 256)?;
                boolean("double_click")?;
                if text("button", 16)?
                    .is_some_and(|value| !["left", "right", "middle"].contains(&value))
                {
                    return Err("invalid mouse button");
                }
            }
            Fill => {
                required("target", 256)?;
                // An empty string is valid when clearing an input.
                if args
                    .get("text")
                    .and_then(Value::as_str)
                    .is_none_or(|value| value.len() > 4096)
                {
                    return Err("invalid fill text");
                }
                text("element", 256)?;
                boolean("slowly")?;
                boolean("submit")?;
            }
            FillForm => {
                let Some(fields) = args.get("fields").and_then(Value::as_array) else {
                    return Err("form fields required");
                };
                if fields.is_empty() || fields.len() > 20 {
                    return Err("invalid form field count");
                }
                for field in fields {
                    let Some(field) = field.as_object() else {
                        return Err("invalid form field");
                    };
                    if field.len() != 4
                        || ["name", "type", "target", "value"]
                            .iter()
                            .any(|key| !field.contains_key(*key))
                    {
                        return Err("invalid form field");
                    }
                    for key in ["name", "type", "target", "value"] {
                        if field[key].as_str().is_none_or(|v| v.len() > 4096) {
                            return Err("invalid form field");
                        }
                    }
                    if field["name"].as_str().is_some_and(str::is_empty)
                        || field["target"].as_str().is_some_and(str::is_empty)
                        || !["textbox", "checkbox", "radio", "combobox", "slider"]
                            .contains(&field["type"].as_str().unwrap_or(""))
                    {
                        return Err("invalid form field");
                    }
                }
            }
            PressKey => {
                required("key", 128)?;
            }
            SelectOption => {
                required("target", 256)?;
                text("element", 256)?;
                let Some(values) = args.get("values").and_then(Value::as_array) else {
                    return Err("option values required");
                };
                if values.is_empty()
                    || values.len() > 20
                    || values
                        .iter()
                        .any(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 256))
                {
                    return Err("invalid option values");
                }
            }
            SetChecked => {
                required("target", 256)?;
                if args.get("checked").and_then(Value::as_bool).is_none() {
                    return Err("checked state required");
                }
            }
            Hover => {
                required("target", 256)?;
                text("element", 256)?;
            }
            Scroll => {
                if !["up", "down"].contains(&required("direction", 8)?) {
                    return Err("invalid scroll direction");
                }
                if args
                    .get("pages")
                    .is_some_and(|v| v.as_u64().is_none_or(|n| n == 0 || n > 10))
                {
                    return Err("invalid scroll pages");
                }
            }
            Drag => {
                required("start_target", 256)?;
                required("end_target", 256)?;
            }
            HandleDialog => {
                if args.get("accept").and_then(Value::as_bool).is_none() {
                    return Err("dialog decision required");
                }
                text("prompt_text", 4096)?;
            }
            FileUpload => {
                let Some(paths) = args.get("paths").and_then(Value::as_array) else {
                    return Err("upload paths required");
                };
                if paths.is_empty()
                    || paths.len() > 10
                    || paths
                        .iter()
                        .any(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 4096))
                {
                    return Err("invalid upload paths");
                }
            }
            RequestHandoff => {
                required("reason", 512)?;
            }
            Pointer => {
                let action = required("action", 16)?;
                if !["click", "move", "wheel"].contains(&action) {
                    return Err("invalid pointer action");
                }
                if action == "wheel" {
                    if args.contains_key("x")
                        || args.contains_key("y")
                        || args.contains_key("button")
                    {
                        return Err("pointer coordinates not applicable");
                    }
                    for key in ["delta_x", "delta_y"] {
                        if args.get(key).is_some_and(|v| {
                            v.as_i64().is_none_or(|n| !(-1000..=1000).contains(&n))
                        }) {
                            return Err("invalid wheel delta");
                        }
                    }
                    if !args.contains_key("delta_x") && !args.contains_key("delta_y") {
                        return Err("wheel delta required");
                    }
                } else {
                    for key in ["x", "y"] {
                        if args
                            .get(key)
                            .and_then(Value::as_u64)
                            .is_none_or(|n| n > 10000)
                        {
                            return Err("pointer coordinates required");
                        }
                    }
                    if args.contains_key("delta_x") || args.contains_key("delta_y") {
                        return Err("wheel delta not applicable");
                    }
                    if action == "click" {
                        if text("button", 16)?
                            .is_some_and(|v| !["left", "right", "middle"].contains(&v))
                        {
                            return Err("invalid pointer button");
                        }
                    } else if args.contains_key("button") {
                        return Err("pointer button not applicable");
                    }
                }
            }
            Drop => {
                required("target", 256)?;
                let paths = args.get("paths").and_then(Value::as_array);
                let data = args.get("data").and_then(Value::as_object);
                if paths.is_none() && data.is_none() {
                    return Err("drop paths or data required");
                }
                if paths.is_some_and(|values| {
                    values.is_empty()
                        || values.len() > 10
                        || values
                            .iter()
                            .any(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 4096))
                }) {
                    return Err("invalid drop paths");
                }
                if data.is_some_and(|values| {
                    values.is_empty()
                        || values.len() > 8
                        || values.iter().any(|(k, v)| {
                            !["text/plain", "text/uri-list"].contains(&k.as_str())
                                || v.as_str().is_none_or(|s| s.len() > 4096)
                        })
                }) {
                    return Err("invalid drop data");
                }
                if args.contains_key("paths") && paths.is_none()
                    || args.contains_key("data") && data.is_none()
                {
                    return Err("invalid drop payload");
                }
            }
            ConsoleMessages => {
                if text("level", 16)?
                    .is_some_and(|v| !["error", "warning", "info", "debug"].contains(&v))
                {
                    return Err("invalid console level");
                }
                boolean("all")?;
            }
            NetworkRequests => {
                boolean("static")?;
                if text("filter", 256)?.is_some_and(|value| {
                    value
                        .chars()
                        .any(|c| !(c.is_ascii_alphanumeric() || "/._:-".contains(c)))
                }) {
                    return Err("network filter must be a plain URL fragment");
                }
            }
            Downloads => {
                if text("filename", 255)?.is_some_and(|name| {
                    name == "."
                        || name == ".."
                        || name
                            .chars()
                            .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
                }) {
                    return Err("invalid download filename");
                }
            }
            GetSession | Close | Snapshot | Screenshot | Resume | PdfSave => {}
        }
        Ok(())
    }

    pub fn persistence_form(&self) -> Self {
        let mut value = self.clone();
        let encoded = serde_json::to_vec(&self.arguments).unwrap_or_default();
        value.arguments =
            serde_json::json!({"digest": format!("blake3:{}", blake3::hash(&encoded))});
        value
    }
}

fn http_url(value: &str) -> Result<(), &'static str> {
    if !(value.starts_with("https://") || value.starts_with("http://"))
        || value.chars().any(char::is_control)
    {
        return Err("only http(s) URLs are allowed");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserResult {
    pub session_ref: Option<RequestId>,
    pub status: String,
    pub data: Value,
    pub execution_identity: crate::ExecutionIdentity,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_inventory_and_sensitive_arguments() {
        assert_eq!(BrowserAction::ALL.len(), 31);
        let request = BrowserRequest {
            action: BrowserAction::Fill,
            session_ref: Some(RequestId::new()),
            arguments: serde_json::json!({"target":"e2","text":"secret"}),
            owner: None,
            pixels_user: None,
        };
        request.validate().unwrap();
        assert!(
            !serde_json::to_string(&request.persistence_form())
                .unwrap()
                .contains("secret")
        );
        assert!(
            BrowserRequest {
                session_ref: None,
                ..request
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn rejects_unsafe_or_ambiguous_browser_arguments() {
        let session_ref = Some(RequestId::new());
        for (action, arguments) in [
            (
                BrowserAction::Navigate,
                serde_json::json!({"url":"file:///secrets"}),
            ),
            (
                BrowserAction::Navigate,
                serde_json::json!({"url":"https://example.com","direction":"back"}),
            ),
            (
                BrowserAction::Find,
                serde_json::json!({"text":"one","regex":"two"}),
            ),
            (
                BrowserAction::Tabs,
                serde_json::json!({"action":"close","index":0}),
            ),
            (
                BrowserAction::Fill,
                serde_json::json!({"target":"e1","text":false}),
            ),
            (
                BrowserAction::Click,
                serde_json::json!({"target":"e1","unknown":"injected"}),
            ),
            (
                BrowserAction::Downloads,
                serde_json::json!({"filename":"../private.txt"}),
            ),
            (
                BrowserAction::Pointer,
                serde_json::json!({"action":"click","x":100}),
            ),
        ] {
            assert!(
                BrowserRequest {
                    action,
                    session_ref,
                    arguments,
                    owner: None,
                    pixels_user: None
                }
                .validate()
                .is_err()
            );
        }
    }
    #[test]
    fn attach_only_accepts_pixels_owned_browser() {
        let valid = [serde_json::json!({}), serde_json::json!({"mode":"pixels"})];
        for arguments in valid {
            BrowserRequest {
                action: BrowserAction::Attach,
                session_ref: None,
                arguments,
                owner: None,
                pixels_user: None,
            }
            .validate()
            .unwrap();
        }
        let invalid = [
            serde_json::json!({"mode":"existing_chrome"}),
            serde_json::json!({"mode":"existing_edge"}),
            serde_json::json!({"mode":"existing_chrome","profile_dir_name":"Default"}),
            serde_json::json!({"profile_dir_name":"Profile 1"}),
            serde_json::json!({"mode":"pixels","profile_dir_name":"Default"}),
            serde_json::json!({"mode":"pixels","user_data_dir":"C:\\Users\\someone"}),
            serde_json::json!({"mode":"unknown"}),
        ];
        for arguments in invalid {
            assert!(
                BrowserRequest {
                    action: BrowserAction::Attach,
                    session_ref: None,
                    arguments,
                    owner: None,
                    pixels_user: None
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn setup_info_reports_only_pixels_browser_availability() {
        for arguments in [serde_json::json!({})] {
            BrowserRequest {
                action: BrowserAction::SetupInfo,
                session_ref: None,
                arguments,
                owner: None,
                pixels_user: None,
            }
            .validate()
            .unwrap();
        }
        for arguments in [
            serde_json::json!({"profile_dir_name":"Default"}),
            serde_json::json!({"user_data_dir":"C:\\Users\\test\\Chrome"}),
            serde_json::json!({"browser":"edge"}),
            serde_json::json!({"browser":"unknown"}),
            serde_json::json!({"unexpected":true}),
        ] {
            assert!(
                BrowserRequest {
                    action: BrowserAction::SetupInfo,
                    session_ref: None,
                    arguments,
                    owner: None,
                    pixels_user: None
                }
                .validate()
                .is_err()
            );
        }
    }
}
