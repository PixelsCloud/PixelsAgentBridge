use crate::RequestId;
use crate::{DesktopAction, DesktopBatchReport, validate_desktop_batch};
use serde::{Deserialize, Serialize};
pub const DESKTOP_HELPER_SCHEMA_VERSION: u16 = 4;
pub const MAX_DESKTOP_TEXT_BYTES: usize = 4096;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopQuery {
    Ui {
        query: crate::UiRequest,
    },
    MonitorInput {
        input: crate::MonitorInput,
    },
    Batch {
        window_ref: String,
        actions: Vec<DesktopAction>,
        timeout_ms: u32,
    },
    Monitors {},
    Windows {},
    Focus {
        window_ref: String,
    },
    Control {
        window_ref: String,
        control: WindowControlAction,
    },
    TypeText {
        window_ref: String,
        text: String,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowControlAction {
    Minimize,
    Maximize,
    Restore,
    Close,
}
impl DesktopQuery {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Ui { query } => query.kind(),
            Self::MonitorInput { .. } => "monitor_input",
            Self::Batch { .. } => "desktop_batch",
            Self::Monitors {} => "monitors",
            Self::Windows {} => "desktop_windows",
            Self::Focus { .. } => "window_focus",
            Self::Control { .. } => "window_control",
            Self::TypeText { .. } => "type_text",
        }
    }
    pub fn is_mutation(&self) -> bool {
        if let Self::Ui { query } = self {
            return query.is_mutation();
        }
        !matches!(self, Self::Monitors {} | Self::Windows {})
    }
    pub fn window_ref(&self) -> Option<&str> {
        match self {
            Self::Batch { window_ref, .. }
            | Self::Focus { window_ref }
            | Self::Control { window_ref, .. }
            | Self::TypeText { window_ref, .. } => Some(window_ref),
            _ => None,
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Self::Ui { query } = self {
            return query.validate();
        }
        if let Self::MonitorInput { input } = self {
            input.validate()?;
        }
        if let Self::Batch {
            actions,
            timeout_ms,
            ..
        } = self
        {
            validate_desktop_batch(actions, *timeout_ms)?;
        }
        if self
            .window_ref()
            .is_some_and(|r| r.parse::<RequestId>().is_err())
        {
            return Err("window_ref must be an opaque reference from a fresh window list");
        }
        if let Self::TypeText { text, .. } = self
            && (text.is_empty() || text.len() > MAX_DESKTOP_TEXT_BYTES || text.contains('\0'))
        {
            return Err("text must be 1..4096 UTF-8 bytes, without NUL");
        }
        Ok(())
    }
    pub fn required_helper_version(&self) -> u16 {
        if matches!(self, Self::Ui { .. }) {
            return 4;
        }
        if matches!(self, Self::MonitorInput { .. }) {
            return 3;
        }
        if matches!(self, Self::Batch { .. }) {
            2
        } else {
            1
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_target: Option<crate::MonitorTarget>,
    pub id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    pub scale_percent: Option<u32>,
    pub rotation_degrees: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopWindowInfo {
    pub window_ref: Option<String>,
    pub title: String,
    pub process_id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub minimized: bool,
    pub maximized: bool,
    pub focused: bool,
    pub monitor_id: Option<u32>,
    pub control_error: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<crate::UiSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<DesktopBatchReport>,
    pub helper_instance: String,
    pub backend: String,
    pub coordinate_space: String,
    pub monitors: Vec<MonitorInfo>,
    pub windows: Vec<DesktopWindowInfo>,
    pub window_ref: Option<String>,
    pub action_started: bool,
    pub verification: Option<String>,
}
impl DesktopSnapshot {
    pub fn new(instance: String, backend: &str) -> Self {
        Self {
            ui: None,
            batch: None,
            helper_instance: instance,
            backend: backend.into(),
            coordinate_space: "xcap_native".into(),
            monitors: vec![],
            windows: vec![],
            window_ref: None,
            action_started: false,
            verification: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SystemQuery;
    #[test]
    fn text_validation_uses_utf8_bytes_and_rejects_nul_and_stale_handle_shapes() {
        let mut q = DesktopQuery::TypeText {
            window_ref: RequestId::new().to_string(),
            text: "中".repeat(1365),
        };
        assert!(q.validate().is_ok());
        if let DesktopQuery::TypeText { text, .. } = &mut q {
            text.push('中');
        }
        assert!(q.validate().is_err());
        for text in ["", "abc\0def"] {
            assert!(
                DesktopQuery::TypeText {
                    window_ref: RequestId::new().to_string(),
                    text: text.into()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            DesktopQuery::Focus {
                window_ref: "1234".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_str::<DesktopQuery>(
                r#"{"operation":"control","window_ref":"x","control":"kill"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<DesktopQuery>(r#"{"operation":"windows","unexpected":true}"#)
                .is_err()
        );
    }
    #[test]
    fn persistence_keeps_identity_without_storing_input_and_capability_is_additive() {
        let q = SystemQuery::Desktop {
            query: DesktopQuery::TypeText {
                window_ref: RequestId::new().to_string(),
                text: "private中文🙂".into(),
            },
        };
        assert_eq!(q.required_version(), 4);
        assert!(q.is_mutation());
        let saved = serde_json::to_string(&q.persistence_form()).unwrap();
        assert!(!saved.contains("private"));
        assert!(saved.contains("blake3:"));
        assert_eq!(
            serde_json::from_str::<SystemQuery>(&saved).unwrap(),
            q.persistence_form()
        );
        assert_eq!(SystemQuery::Disks { limit: 1 }.required_version(), 1);
        assert!(!DesktopQuery::Windows {}.is_mutation());
        assert_eq!(
            SystemQuery::Desktop {
                query: DesktopQuery::Monitors {}
            }
            .kind(),
            "monitors"
        );
    }
}
